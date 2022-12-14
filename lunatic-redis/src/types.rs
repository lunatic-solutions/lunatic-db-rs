use std::collections::{BTreeMap, BTreeSet};
use std::convert::From;
use std::default::Default;
use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::io;
use std::str::{from_utf8, Utf8Error};
use std::string::FromUtf8Error;

#[cfg(feature = "ahash")]
pub(crate) use ahash::{AHashMap as HashMap, AHashSet as HashSet};
use serde::{Deserialize, Serialize};
#[cfg(not(feature = "ahash"))]
pub(crate) use std::collections::{HashMap, HashSet};

macro_rules! invalid_type_error {
    ($v:expr, $det:expr) => {{
        fail!(invalid_type_error_inner!($v, $det))
    }};
}

macro_rules! invalid_type_error_inner {
    ($v:expr, $det:expr) => {
        RedisError::from((
            ErrorKind::TypeError,
            "Response was of incompatible type",
            format!("{:?} (response was {:?})", $det, $v),
        ))
    };
}

/// Helper enum that is used to define expiry time
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Expiry {
    /// EX seconds -- Set the specified expire time, in seconds.
    EX(usize),
    /// PX milliseconds -- Set the specified expire time, in milliseconds.
    PX(usize),
    /// EXAT timestamp-seconds -- Set the specified Unix time at which the key will expire, in seconds.
    EXAT(usize),
    /// PXAT timestamp-milliseconds -- Set the specified Unix time at which the key will expire, in milliseconds.
    PXAT(usize),
    /// PERSIST -- Remove the time to live associated with the key.
    PERSIST,
}

/// Helper enum that is used in some situations to describe
/// the behavior of arguments in a numeric context.
#[derive(PartialEq, Eq, Clone, Debug, Copy, Deserialize, Serialize)]
pub enum NumericBehavior {
    /// This argument is not numeric.
    NonNumeric,
    /// This argument is an integer.
    NumberIsInteger,
    /// This argument is a floating point value.
    NumberIsFloat,
}

/// An enum of all error kinds.
#[derive(PartialEq, Eq, Copy, Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The server generated an invalid response.
    ResponseError,
    /// The authentication with the server failed.
    AuthenticationFailed,
    /// Operation failed because of a type mismatch.
    TypeError,
    /// A script execution was aborted.
    ExecAbortError,
    /// The server cannot response because it's loading a dump.
    BusyLoadingError,
    /// A script that was requested does not actually exist.
    NoScriptError,
    /// An error that was caused because the parameter to the
    /// client were wrong.
    InvalidClientConfig,
    /// Raised if a key moved to a different node.
    Moved,
    /// Raised if a key moved to a different node but we need to ask.
    Ask,
    /// Raised if a request needs to be retried.
    TryAgain,
    /// Raised if a redis cluster is down.
    ClusterDown,
    /// A request spans multiple slots
    CrossSlot,
    /// A cluster master is unavailable.
    MasterDown,
    /// This kind is returned if the redis error is one that is
    /// not native to the system.  This is usually the case if
    /// the cause is another error.
    IoError,
    /// An error raised that was identified on the client before execution.
    ClientError,
    /// An extension error.  This is an error created by the server
    /// that is not directly understood by the library.
    ExtensionError,
    /// Attempt to write to a read-only server
    ReadOnly,
}

/// Internal low-level redis value enum.
#[derive(PartialEq, Eq, Clone, Deserialize, Serialize)]
pub enum Value {
    /// A nil response from the server.
    Nil,
    /// An integer response.  Note that there are a few situations
    /// in which redis actually returns a string for an integer which
    /// is why this library generally treats integers and strings
    /// the same for all numeric responses.
    Int(i64),
    /// An arbitary binary data.
    Data(Vec<u8>),
    /// A bulk response of more data.  This is generally used by redis
    /// to express nested structures.
    Bulk(Vec<Value>),
    /// A status response.
    Status(String),
    /// A status response which represents the string "OK".
    Okay,
}

pub struct MapIter<'a>(std::slice::Iter<'a, Value>);

impl<'a> Iterator for MapIter<'a> {
    type Item = (&'a Value, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        Some((self.0.next()?, self.0.next()?))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (low, high) = self.0.size_hint();
        (low / 2, high.map(|h| h / 2))
    }
}

/// Values are generally not used directly unless you are using the
/// more low level functionality in the library.  For the most part
/// this is hidden with the help of the `FromRedisValue` trait.
///
/// While on the redis protocol there is an error type this is already
/// separated at an early point so the value only holds the remaining
/// types.
impl Value {
    /// Checks if the return value looks like it fulfils the cursor
    /// protocol.  That means the result is a bulk item of length
    /// two with the first one being a cursor and the second a
    /// bulk response.
    pub fn looks_like_cursor(&self) -> bool {
        match *self {
            Value::Bulk(ref items) => {
                if items.len() != 2 {
                    return false;
                }
                match items[0] {
                    Value::Data(_) => {}
                    _ => {
                        return false;
                    }
                };
                match items[1] {
                    Value::Bulk(_) => {}
                    _ => {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Returns an `&[Value]` if `self` is compatible with a sequence type
    pub fn as_sequence(&self) -> Option<&[Value]> {
        match self {
            Value::Bulk(items) => Some(&items[..]),
            Value::Nil => Some(&[]),
            _ => None,
        }
    }

    /// Returns an iterator of `(&Value, &Value)` if `self` is compatible with a map type
    pub fn as_map_iter(&self) -> Option<MapIter<'_>> {
        match self {
            Value::Bulk(items) => Some(MapIter(items.iter())),
            _ => None,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Value::Nil => write!(fmt, "nil"),
            Value::Int(val) => write!(fmt, "int({:?})", val),
            Value::Data(ref val) => match from_utf8(val) {
                Ok(x) => write!(fmt, "string-data('{:?}')", x),
                Err(_) => write!(fmt, "binary-data({:?})", val),
            },
            Value::Bulk(ref values) => {
                write!(fmt, "bulk(")?;
                let mut is_first = true;
                for val in values.iter() {
                    if !is_first {
                        write!(fmt, ", ")?;
                    }
                    write!(fmt, "{:?}", val)?;
                    is_first = false;
                }
                write!(fmt, ")")
            }
            Value::Okay => write!(fmt, "ok"),
            Value::Status(ref s) => write!(fmt, "status({:?})", s),
        }
    }
}

/// Represents a redis error.  For the most part you should be using
/// the Error trait to interact with this rather than the actual
/// struct.
#[derive(Serialize, Deserialize)]
pub struct RedisError {
    repr: ErrorRepr,
}

/// A list specifying general categories of I/O error.
///
/// This list is intended to grow over time and it is not recommended to
/// exhaustively match against it.
///
/// It is used with the [`io::Error`] type.
///
/// [`io::Error`]: Error
///
/// # Handling errors and matching on `ErrorKind`
///
/// In application code, use `match` for the `ErrorKind` values you are
/// expecting; use `_` to match "all other errors".
///
/// In comprehensive and thorough tests that want to verify that a test doesn't
/// return any known incorrect error kind, you may want to cut-and-paste the
/// current full list of errors from here into your test code, and then match
/// `_` as the correct case. This seems counterintuitive, but it will make your
/// tests more robust. In particular, if you want to verify that your code does
/// produce an unrecognized error kind, the robust solution is to check for all
/// the recognized error kinds and fail in those cases.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum IoErrorKind {
    /// An entity was not found, often a file.
    NotFound,
    /// The operation lacked the necessary privileges to complete.
    PermissionDenied,
    /// The connection was refused by the remote server.
    ConnectionRefused,
    /// The connection was reset by the remote server.
    ConnectionReset,
    HostUnreachable,
    NetworkUnreachable,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    /// A nonexistent interface was requested or the requested address was not
    /// local.
    AddrNotAvailable,
    /// The system's networking is down.
    NetworkDown,
    /// The operation failed because a pipe was closed.
    BrokenPipe,
    /// An entity already exists, often a file.
    AlreadyExists,
    /// The operation needs to block to complete, but the blocking operation was
    /// requested to not occur.
    WouldBlock,
    /// A filesystem object is, unexpectedly, not a directory.
    ///
    /// For example, a filesystem path was specified where one of the intermediate directory
    /// components was, in fact, a plain file.
    NotADirectory,
    /// The filesystem object is, unexpectedly, a directory.
    ///
    /// A directory was specified when a non-directory was expected.
    IsADirectory,
    /// A non-empty directory was specified where an empty directory was expected.
    DirectoryNotEmpty,
    /// The filesystem or storage medium is read-only, but a write operation was attempted.
    ReadOnlyFilesystem,
    /// Loop in the filesystem or IO subsystem; often, too many levels of symbolic links.
    ///
    /// There was a loop (or excessively long chain) resolving a filesystem object
    /// or file IO object.
    ///
    /// On Unix this is usually the result of a symbolic link loop; or, of exceeding the
    /// system-specific limit on the depth of symlink traversal.
    FilesystemLoop,
    /// Stale network file handle.
    ///
    /// With some network filesystems, notably NFS, an open file (or directory) can be invalidated
    /// by problems with the network or server.
    StaleNetworkFileHandle,
    /// A parameter was incorrect.
    InvalidInput,
    /// Data not valid for the operation were encountered.
    ///
    /// Unlike [`InvalidInput`], this typically means that the operation
    /// parameters were valid, however the error was caused by malformed
    /// input data.
    ///
    /// For example, a function that reads a file into a string will error with
    /// `InvalidData` if the file's contents are not valid UTF-8.
    ///
    /// [`InvalidInput`]: ErrorKind::InvalidInput
    InvalidData,
    /// The I/O operation's timeout expired, causing it to be canceled.
    TimedOut,
    /// An error returned when an operation could not be completed because a
    /// call to [`write`] returned [`Ok(0)`].
    ///
    /// This typically means that an operation could only succeed if it wrote a
    /// particular number of bytes but only a smaller number of bytes could be
    /// written.
    ///
    /// [`write`]: crate::io::Write::write
    /// [`Ok(0)`]: Ok
    WriteZero,
    /// The underlying storage (typically, a filesystem) is full.
    ///
    /// This does not include out of quota errors.
    StorageFull,
    /// Seek on unseekable file.
    ///
    /// Seeking was attempted on an open file handle which is not suitable for seeking - for
    /// example, on Unix, a named pipe opened with `File::open`.
    NotSeekable,
    /// Filesystem quota was exceeded.
    FilesystemQuotaExceeded,
    /// File larger than allowed or supported.
    ///
    /// This might arise from a hard limit of the underlying filesystem or file access API, or from
    /// an administratively imposed resource limitation.  Simple disk full, and out of quota, have
    /// their own errors.
    FileTooLarge,
    /// Resource is busy.
    ResourceBusy,
    /// Executable file is busy.
    ///
    /// An attempt was made to write to a file which is also in use as a running program.  (Not all
    /// operating systems detect this situation.)
    ExecutableFileBusy,
    /// Deadlock (avoided).
    ///
    /// A file locking operation would result in deadlock.  This situation is typically detected, if
    /// at all, on a best-effort basis.
    Deadlock,
    /// Cross-device or cross-filesystem (hard) link or rename.
    CrossesDevices,
    /// Too many (hard) links to the same filesystem object.
    ///
    /// The filesystem does not support making so many hardlinks to the same file.
    TooManyLinks,
    /// A filename was invalid.
    ///
    /// This error can also cause if it exceeded the filename length limit.
    InvalidFilename,
    /// Program argument list too long.
    ///
    /// When trying to run an external program, a system or process limit on the size of the
    /// arguments would have been exceeded.
    ArgumentListTooLong,
    /// This operation was interrupted.
    ///
    /// Interrupted operations can typically be retried.
    Interrupted,

    /// This operation is unsupported on this platform.
    ///
    /// This means that the operation can never succeed.
    Unsupported,

    // ErrorKinds which are primarily categorisations for OS error
    // codes should be added above.
    //
    /// An error returned when an operation could not be completed because an
    /// "end of file" was reached prematurely.
    ///
    /// This typically means that an operation could only succeed if it read a
    /// particular number of bytes but only a smaller number of bytes could be
    /// read.
    UnexpectedEof,

    /// An operation could not be completed, because it failed
    /// to allocate enough memory.
    OutOfMemory,

    // "Unusual" error kinds which do not correspond simply to (sets
    // of) OS error codes, should be added just above this comment.
    // `Other` and `Uncategorised` should remain at the end:
    //
    /// A custom error that does not fall under any other I/O error kind.
    ///
    /// This can be used to construct your own [`Error`]s that do not match any
    /// [`ErrorKind`].
    ///
    /// This [`ErrorKind`] is not used by the standard library.
    ///
    /// Errors from the standard library that do not fall under any of the I/O
    /// error kinds cannot be `match`ed on, and will only match a wildcard (`_`) pattern.
    /// New [`ErrorKind`]s might be added in the future for some of those.
    Other,

    /// Any I/O error from the standard library that's not part of this list.
    ///
    /// Errors that are `Uncategorized` now may move to a different or a new
    /// [`ErrorKind`] variant in the future. It is not recommended to match
    /// an error against `Uncategorized`; use a wildcard match (`_`) instead.
    Uncategorized,
}

impl From<io::ErrorKind> for IoErrorKind {
    fn from(kind: io::ErrorKind) -> Self {
        match kind {
            io::ErrorKind::NotFound => IoErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => IoErrorKind::PermissionDenied,
            io::ErrorKind::ConnectionRefused => IoErrorKind::ConnectionRefused,
            io::ErrorKind::ConnectionReset => IoErrorKind::ConnectionReset,
            // io::ErrorKind::HostUnreachable => IoErrorKind::HostUnreachable,
            // io::ErrorKind::NetworkUnreachable => IoErrorKind::NetworkUnreachable,
            io::ErrorKind::ConnectionAborted => IoErrorKind::ConnectionAborted,
            io::ErrorKind::NotConnected => IoErrorKind::NotConnected,
            io::ErrorKind::AddrInUse => IoErrorKind::AddrInUse,
            io::ErrorKind::AddrNotAvailable => IoErrorKind::AddrNotAvailable,
            // io::ErrorKind::NetworkDown => IoErrorKind::NetworkDown,
            io::ErrorKind::BrokenPipe => IoErrorKind::BrokenPipe,
            io::ErrorKind::AlreadyExists => IoErrorKind::AlreadyExists,
            io::ErrorKind::WouldBlock => IoErrorKind::WouldBlock,
            // io::ErrorKind::NotADirectory => IoErrorKind::NotADirectory,
            // io::ErrorKind::IsADirectory => IoErrorKind::IsADirectory,
            // io::ErrorKind::DirectoryNotEmpty => IoErrorKind::DirectoryNotEmpty,
            // io::ErrorKind::ReadOnlyFilesystem => IoErrorKind::ReadOnlyFilesystem,
            // io::ErrorKind::FilesystemLoop => IoErrorKind::FilesystemLoop,
            // io::ErrorKind::StaleNetworkFileHandle => IoErrorKind::StaleNetworkFileHandle,
            io::ErrorKind::InvalidInput => IoErrorKind::InvalidInput,
            io::ErrorKind::InvalidData => IoErrorKind::InvalidData,
            io::ErrorKind::TimedOut => IoErrorKind::TimedOut,
            io::ErrorKind::WriteZero => IoErrorKind::WriteZero,
            // io::ErrorKind::StorageFull => IoErrorKind::StorageFull,
            // io::ErrorKind::NotSeekable => IoErrorKind::NotSeekable,
            // io::ErrorKind::FilesystemQuotaExceeded => IoErrorKind::FilesystemQuotaExceeded,
            // io::ErrorKind::FileTooLarge => IoErrorKind::FileTooLarge,
            // io::ErrorKind::ResourceBusy => IoErrorKind::ResourceBusy,
            // io::ErrorKind::ExecutableFileBusy => IoErrorKind::ExecutableFileBusy,
            // io::ErrorKind::Deadlock => IoErrorKind::Deadlock,
            // io::ErrorKind::CrossesDevices => IoErrorKind::CrossesDevices,
            // io::ErrorKind::TooManyLinks => IoErrorKind::TooManyLinks,
            // io::ErrorKind::InvalidFilename => IoErrorKind::InvalidFilename,
            // io::ErrorKind::ArgumentListTooLong => IoErrorKind::ArgumentListTooLong,
            io::ErrorKind::Interrupted => IoErrorKind::Interrupted,
            io::ErrorKind::Unsupported => IoErrorKind::Unsupported,
            io::ErrorKind::UnexpectedEof => IoErrorKind::UnexpectedEof,
            io::ErrorKind::OutOfMemory => IoErrorKind::OutOfMemory,
            io::ErrorKind::Other => IoErrorKind::Other,
            // io::ErrorKind::Uncategorized => IoErrorKind::Uncategorized,
            _ => todo!(),
        }
    }
}

impl IoErrorKind {
    pub(crate) fn as_str(&self) -> &'static str {
        use IoErrorKind::*;
        // Strictly alphabetical, please.  (Sadly rustfmt cannot do this yet.)
        match *self {
            AddrInUse => "address in use",
            AddrNotAvailable => "address not available",
            AlreadyExists => "entity already exists",
            ArgumentListTooLong => "argument list too long",
            BrokenPipe => "broken pipe",
            ConnectionAborted => "connection aborted",
            ConnectionRefused => "connection refused",
            ConnectionReset => "connection reset",
            CrossesDevices => "cross-device link or rename",
            Deadlock => "deadlock",
            DirectoryNotEmpty => "directory not empty",
            ExecutableFileBusy => "executable file busy",
            FileTooLarge => "file too large",
            FilesystemLoop => "filesystem loop or indirection limit (e.g. symlink loop)",
            FilesystemQuotaExceeded => "filesystem quota exceeded",
            HostUnreachable => "host unreachable",
            Interrupted => "operation interrupted",
            InvalidData => "invalid data",
            InvalidFilename => "invalid filename",
            InvalidInput => "invalid input parameter",
            IsADirectory => "is a directory",
            NetworkDown => "network down",
            NetworkUnreachable => "network unreachable",
            NotADirectory => "not a directory",
            NotConnected => "not connected",
            NotFound => "entity not found",
            NotSeekable => "seek on unseekable file",
            Other => "other error",
            OutOfMemory => "out of memory",
            PermissionDenied => "permission denied",
            ReadOnlyFilesystem => "read-only filesystem or storage medium",
            ResourceBusy => "resource busy",
            StaleNetworkFileHandle => "stale network file handle",
            StorageFull => "no storage space",
            TimedOut => "timed out",
            TooManyLinks => "too many links",
            Uncategorized => "uncategorized error",
            UnexpectedEof => "unexpected end of file",
            Unsupported => "unsupported",
            WouldBlock => "operation would block",
            WriteZero => "write zero",
        }
    }
}

impl fmt::Display for IoErrorKind {
    /// Shows a human-readable description of the `ErrorKind`.
    ///
    /// This is similar to `impl Display for Error`, but doesn't require first converting to Error.
    ///
    /// # Examples
    /// ```
    /// use std::io::ErrorKind;
    /// assert_eq!("entity not found", ErrorKind::NotFound.to_string());
    /// ```
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str(self.as_str())
    }
}

#[derive(Debug, Deserialize, Serialize)]
enum ErrorRepr {
    WithDescription(ErrorKind, String),
    WithDescriptionAndDetail(ErrorKind, String, String),
    ExtensionError(String, String),
    IoError(IoErrorKind, String),
}

impl PartialEq for RedisError {
    fn eq(&self, other: &RedisError) -> bool {
        match (&self.repr, &other.repr) {
            (&ErrorRepr::WithDescription(kind_a, _), &ErrorRepr::WithDescription(kind_b, _)) => {
                kind_a == kind_b
            }
            (
                &ErrorRepr::WithDescriptionAndDetail(kind_a, _, _),
                &ErrorRepr::WithDescriptionAndDetail(kind_b, _, _),
            ) => kind_a == kind_b,
            (&ErrorRepr::ExtensionError(ref a, _), &ErrorRepr::ExtensionError(ref b, _)) => {
                *a == *b
            }
            _ => false,
        }
    }
}

impl From<io::Error> for RedisError {
    fn from(err: io::Error) -> RedisError {
        RedisError {
            repr: ErrorRepr::IoError(IoErrorKind::from(err.kind()), err.to_string()),
        }
    }
}

impl From<Utf8Error> for RedisError {
    fn from(_: Utf8Error) -> RedisError {
        RedisError {
            repr: ErrorRepr::WithDescription(ErrorKind::TypeError, "Invalid UTF-8".to_string()),
        }
    }
}

impl From<FromUtf8Error> for RedisError {
    fn from(_: FromUtf8Error) -> RedisError {
        RedisError {
            repr: ErrorRepr::WithDescription(
                ErrorKind::TypeError,
                "Cannot convert from UTF-8".to_string(),
            ),
        }
    }
}

impl From<(ErrorKind, &'static str)> for RedisError {
    fn from((kind, desc): (ErrorKind, &'static str)) -> RedisError {
        RedisError {
            repr: ErrorRepr::WithDescription(kind, desc.to_string()),
        }
    }
}

impl From<(ErrorKind, &'static str, String)> for RedisError {
    fn from((kind, desc, detail): (ErrorKind, &'static str, String)) -> RedisError {
        RedisError {
            repr: ErrorRepr::WithDescriptionAndDetail(kind, desc.to_string(), detail),
        }
    }
}

// impl error::Error for RedisError {
//     #[allow(deprecated)]
//     fn description(&self) -> &str {
//         match self.repr {
//             ErrorRepr::WithDescription(_, desc) => &desc.clone(),
//             ErrorRepr::WithDescriptionAndDetail(_, desc, _) => &desc.clone(),
//             ErrorRepr::ExtensionError(_, _) => "extension error",
//             ErrorRepr::IoError(_, description) => &description.clone(),
//         }
//     }

//     fn cause(&self) -> Option<&dyn error::Error> {
//         match self.repr {
//             // ErrorRepr::IoError(ref err) => Some(err as &dyn error::Error),
//             _ => None,
//         }
//     }
// }

impl fmt::Display for RedisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match &self.repr {
            ErrorRepr::WithDescription(_, desc) => desc.fmt(f),
            ErrorRepr::WithDescriptionAndDetail(_, desc, ref detail) => {
                desc.fmt(f)?;
                f.write_str(": ")?;
                detail.fmt(f)
            }
            ErrorRepr::ExtensionError(ref code, ref detail) => {
                code.fmt(f)?;
                f.write_str(": ")?;
                detail.fmt(f)
            }
            ErrorRepr::IoError(kind, desc) => {
                write!(f, "{}: {}", kind, desc)?;
                Ok(())
            }
        }
    }
}

impl fmt::Debug for RedisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        fmt::Display::fmt(self, f)
    }
}

/// Indicates a general failure in the library.
impl RedisError {
    /// Returns the kind of the error.
    pub fn kind(&self) -> ErrorKind {
        match self.repr {
            ErrorRepr::WithDescription(kind, _)
            | ErrorRepr::WithDescriptionAndDetail(kind, _, _) => kind,
            ErrorRepr::ExtensionError(_, _) => ErrorKind::ExtensionError,
            ErrorRepr::IoError(_, _) => ErrorKind::IoError,
        }
    }

    /// Returns the error detail.
    pub fn detail(&self) -> Option<&str> {
        match self.repr {
            ErrorRepr::WithDescriptionAndDetail(_, _, ref detail)
            | ErrorRepr::ExtensionError(_, ref detail) => Some(detail.as_str()),
            _ => None,
        }
    }

    /// Returns the raw error code if available.
    pub fn code(&self) -> Option<&str> {
        match self.kind() {
            ErrorKind::ResponseError => Some("ERR"),
            ErrorKind::ExecAbortError => Some("EXECABORT"),
            ErrorKind::BusyLoadingError => Some("LOADING"),
            ErrorKind::NoScriptError => Some("NOSCRIPT"),
            ErrorKind::Moved => Some("MOVED"),
            ErrorKind::Ask => Some("ASK"),
            ErrorKind::TryAgain => Some("TRYAGAIN"),
            ErrorKind::ClusterDown => Some("CLUSTERDOWN"),
            ErrorKind::CrossSlot => Some("CROSSSLOT"),
            ErrorKind::MasterDown => Some("MASTERDOWN"),
            ErrorKind::ReadOnly => Some("READONLY"),
            _ => match self.repr {
                ErrorRepr::ExtensionError(ref code, _) => Some(code),
                _ => None,
            },
        }
    }

    /// Returns the name of the error category for display purposes.
    pub fn category(&self) -> &str {
        match self.kind() {
            ErrorKind::ResponseError => "response error",
            ErrorKind::AuthenticationFailed => "authentication failed",
            ErrorKind::TypeError => "type error",
            ErrorKind::ExecAbortError => "script execution aborted",
            ErrorKind::BusyLoadingError => "busy loading",
            ErrorKind::NoScriptError => "no script",
            ErrorKind::InvalidClientConfig => "invalid client config",
            ErrorKind::Moved => "key moved",
            ErrorKind::Ask => "key moved (ask)",
            ErrorKind::TryAgain => "try again",
            ErrorKind::ClusterDown => "cluster down",
            ErrorKind::CrossSlot => "cross-slot",
            ErrorKind::MasterDown => "master down",
            ErrorKind::IoError => "I/O error",
            ErrorKind::ExtensionError => "extension error",
            ErrorKind::ClientError => "client error",
            ErrorKind::ReadOnly => "read-only",
        }
    }

    /// Indicates that this failure is an IO failure.
    pub fn is_io_error(&self) -> bool {
        self.as_io_error().is_some()
    }

    // TODO: implement mapping of custom IoErrorKind type to io::ErrorKind
    pub(crate) fn as_io_error(&self) -> Option<&io::Error> {
        None
        // match &self.repr {
        //     ErrorRepr::IoError(kind, desc) => Some(&io::Error::new(kind.into(), desc)),
        //     _ => None,
        // }
    }

    /// Indicates that this is a cluster error.
    pub fn is_cluster_error(&self) -> bool {
        matches!(
            self.kind(),
            ErrorKind::Moved | ErrorKind::Ask | ErrorKind::TryAgain | ErrorKind::ClusterDown
        )
    }

    /// Returns true if this error indicates that the connection was
    /// refused.  You should generally not rely much on this function
    /// unless you are writing unit tests that want to detect if a
    /// local server is available.
    pub fn is_connection_refusal(&self) -> bool {
        match &self.repr {
            ErrorRepr::IoError(kind, _desc) =>
            {
                #[allow(clippy::match_like_matches_macro)]
                match kind {
                    IoErrorKind::ConnectionRefused => true,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Returns true if error was caused by I/O time out.
    /// Note that this may not be accurate depending on platform.
    pub fn is_timeout(&self) -> bool {
        match &self.repr {
            ErrorRepr::IoError(kind, _desc) => {
                matches!(kind, IoErrorKind::TimedOut | IoErrorKind::WouldBlock)
            }
            _ => false,
        }
    }

    /// Returns true if error was caused by a dropped connection.
    pub fn is_connection_dropped(&self) -> bool {
        match &self.repr {
            ErrorRepr::IoError(kind, _desc) => {
                matches!(kind, IoErrorKind::BrokenPipe | IoErrorKind::ConnectionReset)
            }
            _ => false,
        }
    }

    /// Returns the node the error refers to.
    ///
    /// This returns `(addr, slot_id)`.
    pub fn redirect_node(&self) -> Option<(&str, u16)> {
        match self.kind() {
            ErrorKind::Ask | ErrorKind::Moved => (),
            _ => return None,
        }
        let mut iter = self.detail()?.split_ascii_whitespace();
        let slot_id: u16 = iter.next()?.parse().ok()?;
        let addr = iter.next()?;
        Some((addr, slot_id))
    }

    /// Returns the extension error code.
    ///
    /// This method should not be used because every time the redis library
    /// adds support for a new error code it would disappear form this method.
    /// `code()` always returns the code.
    #[deprecated(note = "use code() instead")]
    pub fn extension_error_code(&self) -> Option<&str> {
        match self.repr {
            ErrorRepr::ExtensionError(ref code, _) => Some(code),
            _ => None,
        }
    }

    /// Clone the `RedisError`, throwing away non-cloneable parts of an `IoError`.
    ///
    /// Deriving `Clone` is not possible because the wrapped `io::Error` is not
    /// cloneable.
    ///
    /// The `ioerror_description` parameter will be prepended to the message in
    /// case an `IoError` is found.
    #[cfg(feature = "connection-manager")] // Used to avoid "unused method" warning
    pub(crate) fn clone_mostly(&self, ioerror_description: &'static str) -> Self {
        let repr = match self.repr {
            ErrorRepr::WithDescription(kind, desc) => ErrorRepr::WithDescription(kind, desc),
            ErrorRepr::WithDescriptionAndDetail(kind, desc, ref detail) => {
                ErrorRepr::WithDescriptionAndDetail(kind, desc, detail.clone())
            }
            ErrorRepr::ExtensionError(ref code, ref detail) => {
                ErrorRepr::ExtensionError(code.clone(), detail.clone())
            }
            ErrorRepr::IoError(kind, desc) => ErrorRepr::IoError(kind.clone(), desc.clone()),
        };
        Self { repr }
    }
}

pub fn make_extension_error(code: &str, detail: Option<&str>) -> RedisError {
    RedisError {
        repr: ErrorRepr::ExtensionError(
            code.to_string(),
            match detail {
                Some(x) => x.to_string(),
                None => "Unknown extension error encountered".to_string(),
            },
        ),
    }
}

/// Library generic result type.
pub type RedisResult<T> = Result<T, RedisError>;

/// An info dictionary type.
#[derive(Debug, Deserialize, Serialize)]
pub struct InfoDict {
    map: HashMap<String, Value>,
}

/// This type provides convenient access to key/value data returned by
/// the "INFO" command.  It acts like a regular mapping but also has
/// a convenience method `get` which can return data in the appropriate
/// type.
///
/// For instance this can be used to query the server for the role it's
/// in (master, slave) etc:
///
/// ```rust,no_run
/// # fn do_something() -> redis::RedisResult<()> {
/// # let client = redis::Client::open("redis://127.0.0.1/").unwrap();
/// # let mut con = client.get_connection().unwrap();
/// let info : redis::InfoDict = redis::cmd("INFO").query(&mut con)?;
/// let role : Option<String> = info.get("role");
/// # Ok(()) }
/// ```
impl InfoDict {
    /// Creates a new info dictionary from a string in the response of
    /// the INFO command.  Each line is a key, value pair with the
    /// key and value separated by a colon (`:`).  Lines starting with a
    /// hash (`#`) are ignored.
    pub fn new(kvpairs: &str) -> InfoDict {
        let mut map = HashMap::new();
        for line in kvpairs.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut p = line.splitn(2, ':');
            let k = unwrap_or!(p.next(), continue).to_string();
            let v = unwrap_or!(p.next(), continue).to_string();
            map.insert(k, Value::Status(v));
        }
        InfoDict { map }
    }

    /// Fetches a value by key and converts it into the given type.
    /// Typical types are `String`, `bool` and integer types.
    pub fn get<T: FromRedisValue>(&self, key: &str) -> Option<T> {
        match self.find(&key) {
            Some(x) => from_redis_value(x).ok(),
            None => None,
        }
    }

    /// Looks up a key in the info dict.
    pub fn find(&self, key: &&str) -> Option<&Value> {
        self.map.get(*key)
    }

    /// Checks if a key is contained in the info dicf.
    pub fn contains_key(&self, key: &&str) -> bool {
        self.find(key).is_some()
    }

    /// Returns the size of the info dict.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Checks if the dict is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Abstraction trait for redis command abstractions.
pub trait RedisWrite {
    /// Accepts a serialized redis command.
    fn write_arg(&mut self, arg: &[u8]);

    /// Accepts a serialized redis command.
    fn write_arg_fmt(&mut self, arg: impl fmt::Display) {
        self.write_arg(arg.to_string().as_bytes())
    }
}

impl RedisWrite for Vec<Vec<u8>> {
    fn write_arg(&mut self, arg: &[u8]) {
        self.push(arg.to_owned());
    }

    fn write_arg_fmt(&mut self, arg: impl fmt::Display) {
        self.push(arg.to_string().into_bytes())
    }
}

/// Used to convert a value into one or multiple redis argument
/// strings.  Most values will produce exactly one item but in
/// some cases it might make sense to produce more than one.
pub trait ToRedisArgs: Sized {
    /// This converts the value into a vector of bytes.  Each item
    /// is a single argument.  Most items generate a vector of a
    /// single item.
    ///
    /// The exception to this rule currently are vectors of items.
    fn to_redis_args(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        self.write_redis_args(&mut out);
        out
    }

    /// This writes the value into a vector of bytes.  Each item
    /// is a single argument.  Most items generate a single item.
    ///
    /// The exception to this rule currently are vectors of items.
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite;

    /// Returns an information about the contained value with regards
    /// to it's numeric behavior in a redis context.  This is used in
    /// some high level concepts to switch between different implementations
    /// of redis functions (for instance `INCR` vs `INCRBYFLOAT`).
    fn describe_numeric_behavior(&self) -> NumericBehavior {
        NumericBehavior::NonNumeric
    }

    /// Returns an indiciation if the value contained is exactly one
    /// argument.  It returns false if it's zero or more than one.  This
    /// is used in some high level functions to intelligently switch
    /// between `GET` and `MGET` variants.
    fn is_single_arg(&self) -> bool {
        true
    }

    /// This only exists internally as a workaround for the lack of
    /// specialization.
    #[doc(hidden)]
    fn make_arg_vec<W>(items: &[Self], out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        for item in items.iter() {
            item.write_redis_args(out);
        }
    }

    /// This only exists internally as a workaround for the lack of
    /// specialization.
    #[doc(hidden)]
    fn make_arg_iter_ref<'a, I, W>(items: I, out: &mut W)
    where
        W: ?Sized + RedisWrite,
        I: Iterator<Item = &'a Self>,
        Self: 'a,
    {
        for item in items {
            item.write_redis_args(out);
        }
    }

    #[doc(hidden)]
    fn is_single_vec_arg(items: &[Self]) -> bool {
        items.len() == 1 && items[0].is_single_arg()
    }
}

macro_rules! itoa_based_to_redis_impl {
    ($t:ty, $numeric:expr) => {
        impl ToRedisArgs for $t {
            fn write_redis_args<W>(&self, out: &mut W)
            where
                W: ?Sized + RedisWrite,
            {
                let mut buf = ::itoa::Buffer::new();
                let s = buf.format(*self);
                out.write_arg(s.as_bytes())
            }

            fn describe_numeric_behavior(&self) -> NumericBehavior {
                $numeric
            }
        }
    };
}

macro_rules! non_zero_itoa_based_to_redis_impl {
    ($t:ty, $numeric:expr) => {
        impl ToRedisArgs for $t {
            fn write_redis_args<W>(&self, out: &mut W)
            where
                W: ?Sized + RedisWrite,
            {
                let mut buf = ::itoa::Buffer::new();
                let s = buf.format(self.get());
                out.write_arg(s.as_bytes())
            }

            fn describe_numeric_behavior(&self) -> NumericBehavior {
                $numeric
            }
        }
    };
}

macro_rules! ryu_based_to_redis_impl {
    ($t:ty, $numeric:expr) => {
        impl ToRedisArgs for $t {
            fn write_redis_args<W>(&self, out: &mut W)
            where
                W: ?Sized + RedisWrite,
            {
                let mut buf = ::ryu::Buffer::new();
                let s = buf.format(*self);
                out.write_arg(s.as_bytes())
            }

            fn describe_numeric_behavior(&self) -> NumericBehavior {
                $numeric
            }
        }
    };
}

impl ToRedisArgs for u8 {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        let mut buf = ::itoa::Buffer::new();
        let s = buf.format(*self);
        out.write_arg(s.as_bytes())
    }

    fn make_arg_vec<W>(items: &[u8], out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        out.write_arg(items);
    }

    fn is_single_vec_arg(_items: &[u8]) -> bool {
        true
    }
}

itoa_based_to_redis_impl!(i8, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(i16, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(u16, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(i32, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(u32, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(i64, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(u64, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(isize, NumericBehavior::NumberIsInteger);
itoa_based_to_redis_impl!(usize, NumericBehavior::NumberIsInteger);

non_zero_itoa_based_to_redis_impl!(core::num::NonZeroU8, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroI8, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroU16, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroI16, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroU32, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroI32, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroU64, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroI64, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroUsize, NumericBehavior::NumberIsInteger);
non_zero_itoa_based_to_redis_impl!(core::num::NonZeroIsize, NumericBehavior::NumberIsInteger);

ryu_based_to_redis_impl!(f32, NumericBehavior::NumberIsFloat);
ryu_based_to_redis_impl!(f64, NumericBehavior::NumberIsFloat);

impl ToRedisArgs for bool {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        out.write_arg(if *self { b"1" } else { b"0" })
    }
}

impl ToRedisArgs for String {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        out.write_arg(self.as_bytes())
    }
}

impl<'a> ToRedisArgs for &'a str {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        out.write_arg(self.as_bytes())
    }
}

impl<T: ToRedisArgs> ToRedisArgs for Vec<T> {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        ToRedisArgs::make_arg_vec(self, out)
    }

    fn is_single_arg(&self) -> bool {
        ToRedisArgs::is_single_vec_arg(&self[..])
    }
}

impl<'a, T: ToRedisArgs> ToRedisArgs for &'a [T] {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        ToRedisArgs::make_arg_vec(*self, out)
    }

    fn is_single_arg(&self) -> bool {
        ToRedisArgs::is_single_vec_arg(*self)
    }
}

impl<T: ToRedisArgs> ToRedisArgs for Option<T> {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        if let Some(ref x) = *self {
            x.write_redis_args(out);
        }
    }

    fn describe_numeric_behavior(&self) -> NumericBehavior {
        match *self {
            Some(ref x) => x.describe_numeric_behavior(),
            None => NumericBehavior::NonNumeric,
        }
    }

    fn is_single_arg(&self) -> bool {
        match *self {
            Some(ref x) => x.is_single_arg(),
            None => false,
        }
    }
}

impl<T: ToRedisArgs> ToRedisArgs for &T {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        (*self).write_redis_args(out)
    }

    fn is_single_arg(&self) -> bool {
        (*self).is_single_arg()
    }
}

/// @note: Redis cannot store empty sets so the application has to
/// check whether the set is empty and if so, not attempt to use that
/// result
impl<T: ToRedisArgs + Hash + Eq, S: BuildHasher + Default> ToRedisArgs
    for std::collections::HashSet<T, S>
{
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        ToRedisArgs::make_arg_iter_ref(self.iter(), out)
    }

    fn is_single_arg(&self) -> bool {
        self.len() <= 1
    }
}

/// @note: Redis cannot store empty sets so the application has to
/// check whether the set is empty and if so, not attempt to use that
/// result
#[cfg(feature = "ahash")]
impl<T: ToRedisArgs + Hash + Eq, S: BuildHasher + Default> ToRedisArgs for ahash::AHashSet<T, S> {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        ToRedisArgs::make_arg_iter_ref(self.iter(), out)
    }

    fn is_single_arg(&self) -> bool {
        self.len() <= 1
    }
}

/// @note: Redis cannot store empty sets so the application has to
/// check whether the set is empty and if so, not attempt to use that
/// result
impl<T: ToRedisArgs + Hash + Eq + Ord> ToRedisArgs for BTreeSet<T> {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        ToRedisArgs::make_arg_iter_ref(self.iter(), out)
    }

    fn is_single_arg(&self) -> bool {
        self.len() <= 1
    }
}

/// this flattens BTreeMap into something that goes well with HMSET
/// @note: Redis cannot store empty sets so the application has to
/// check whether the set is empty and if so, not attempt to use that
/// result
impl<T: ToRedisArgs + Hash + Eq + Ord, V: ToRedisArgs> ToRedisArgs for BTreeMap<T, V> {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        for (key, value) in self {
            // otherwise things like HMSET will simply NOT work
            assert!(key.is_single_arg() && value.is_single_arg());

            key.write_redis_args(out);
            value.write_redis_args(out);
        }
    }

    fn is_single_arg(&self) -> bool {
        self.len() <= 1
    }
}

macro_rules! to_redis_args_for_tuple {
    () => ();
    ($($name:ident,)+) => (
        #[doc(hidden)]
        impl<$($name: ToRedisArgs),*> ToRedisArgs for ($($name,)*) {
            // we have local variables named T1 as dummies and those
            // variables are unused.
            #[allow(non_snake_case, unused_variables)]
            fn write_redis_args<W>(&self, out: &mut W) where W: ?Sized + RedisWrite {
                let ($(ref $name,)*) = *self;
                $($name.write_redis_args(out);)*
            }

            #[allow(non_snake_case, unused_variables)]
            fn is_single_arg(&self) -> bool {
                let mut n = 0u32;
                $(let $name = (); n += 1;)*
                n == 1
            }
        }
        to_redis_args_for_tuple_peel!($($name,)*);
    )
}

/// This chips of the leading one and recurses for the rest.  So if the first
/// iteration was T1, T2, T3 it will recurse to T2, T3.  It stops for tuples
/// of size 1 (does not implement down to unit).
macro_rules! to_redis_args_for_tuple_peel {
    ($name:ident, $($other:ident,)*) => (to_redis_args_for_tuple!($($other,)*);)
}

to_redis_args_for_tuple! { T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, }

macro_rules! to_redis_args_for_array {
    ($($N:expr)+) => {
        $(
            impl<'a, T: ToRedisArgs> ToRedisArgs for &'a [T; $N] {
                fn write_redis_args<W>(&self, out: &mut W) where W: ?Sized + RedisWrite {
                    ToRedisArgs::make_arg_vec(*self, out)
                }

                fn is_single_arg(&self) -> bool {
                    ToRedisArgs::is_single_vec_arg(*self)
                }
            }
        )+
    }
}

to_redis_args_for_array! {
     0  1  2  3  4  5  6  7  8  9
    10 11 12 13 14 15 16 17 18 19
    20 21 22 23 24 25 26 27 28 29
    30 31 32
}

/// This trait is used to convert a redis value into a more appropriate
/// type.  While a redis `Value` can represent any response that comes
/// back from the redis server, usually you want to map this into something
/// that works better in rust.  For instance you might want to convert the
/// return value into a `String` or an integer.
///
/// This trait is well supported throughout the library and you can
/// implement it for your own types if you want.
///
/// In addition to what you can see from the docs, this is also implemented
/// for tuples up to size 12 and for Vec<u8>.
pub trait FromRedisValue: Sized {
    /// Given a redis `Value` this attempts to convert it into the given
    /// destination type.  If that fails because it's not compatible an
    /// appropriate error is generated.
    fn from_redis_value(v: &Value) -> RedisResult<Self>;

    /// Similar to `from_redis_value` but constructs a vector of objects
    /// from another vector of values.  This primarily exists internally
    /// to customize the behavior for vectors of tuples.
    fn from_redis_values(items: &[Value]) -> RedisResult<Vec<Self>> {
        items.iter().map(FromRedisValue::from_redis_value).collect()
    }

    /// This only exists internally as a workaround for the lack of
    /// specialization.
    #[doc(hidden)]
    fn from_byte_vec(_vec: &[u8]) -> Option<Vec<Self>> {
        None
    }
}

macro_rules! from_redis_value_for_num_internal {
    ($t:ty, $v:expr) => {{
        let v = $v;
        match *v {
            Value::Int(val) => Ok(val as $t),
            Value::Status(ref s) => match s.parse::<$t>() {
                Ok(rv) => Ok(rv),
                Err(_) => invalid_type_error!(v, "Could not convert from string."),
            },
            Value::Data(ref bytes) => match from_utf8(bytes)?.parse::<$t>() {
                Ok(rv) => Ok(rv),
                Err(_) => invalid_type_error!(v, "Could not convert from string."),
            },
            _ => invalid_type_error!(v, "Response type not convertible to numeric."),
        }
    }};
}

macro_rules! from_redis_value_for_num {
    ($t:ty) => {
        impl FromRedisValue for $t {
            fn from_redis_value(v: &Value) -> RedisResult<$t> {
                from_redis_value_for_num_internal!($t, v)
            }
        }
    };
}

impl FromRedisValue for u8 {
    fn from_redis_value(v: &Value) -> RedisResult<u8> {
        from_redis_value_for_num_internal!(u8, v)
    }

    fn from_byte_vec(vec: &[u8]) -> Option<Vec<u8>> {
        Some(vec.to_vec())
    }
}

from_redis_value_for_num!(i8);
from_redis_value_for_num!(i16);
from_redis_value_for_num!(u16);
from_redis_value_for_num!(i32);
from_redis_value_for_num!(u32);
from_redis_value_for_num!(i64);
from_redis_value_for_num!(u64);
from_redis_value_for_num!(i128);
from_redis_value_for_num!(u128);
from_redis_value_for_num!(f32);
from_redis_value_for_num!(f64);
from_redis_value_for_num!(isize);
from_redis_value_for_num!(usize);

impl FromRedisValue for bool {
    fn from_redis_value(v: &Value) -> RedisResult<bool> {
        match *v {
            Value::Nil => Ok(false),
            Value::Int(val) => Ok(val != 0),
            Value::Status(ref s) => {
                if &s[..] == "1" {
                    Ok(true)
                } else if &s[..] == "0" {
                    Ok(false)
                } else {
                    invalid_type_error!(v, "Response status not valid boolean");
                }
            }
            Value::Data(ref bytes) => {
                if bytes == b"1" {
                    Ok(true)
                } else if bytes == b"0" {
                    Ok(false)
                } else {
                    invalid_type_error!(v, "Response type not bool compatible.");
                }
            }
            Value::Okay => Ok(true),
            _ => invalid_type_error!(v, "Response type not bool compatible."),
        }
    }
}

impl FromRedisValue for String {
    fn from_redis_value(v: &Value) -> RedisResult<String> {
        match *v {
            Value::Data(ref bytes) => Ok(from_utf8(bytes)?.to_string()),
            Value::Okay => Ok("OK".to_string()),
            Value::Status(ref val) => Ok(val.to_string()),
            _ => invalid_type_error!(v, "Response type not string compatible."),
        }
    }
}

impl<T: FromRedisValue> FromRedisValue for Vec<T> {
    fn from_redis_value(v: &Value) -> RedisResult<Vec<T>> {
        match *v {
            // this hack allows us to specialize Vec<u8> to work with
            // binary data whereas all others will fail with an error.
            Value::Data(ref bytes) => match FromRedisValue::from_byte_vec(bytes) {
                Some(x) => Ok(x),
                None => invalid_type_error!(v, "Response type not vector compatible."),
            },
            Value::Bulk(ref items) => FromRedisValue::from_redis_values(items),
            Value::Nil => Ok(vec![]),
            _ => invalid_type_error!(v, "Response type not vector compatible."),
        }
    }
}

impl<K: FromRedisValue + Eq + Hash, V: FromRedisValue, S: BuildHasher + Default> FromRedisValue
    for std::collections::HashMap<K, V, S>
{
    fn from_redis_value(v: &Value) -> RedisResult<std::collections::HashMap<K, V, S>> {
        v.as_map_iter()
            .ok_or_else(|| invalid_type_error_inner!(v, "Response type not hashmap compatible"))?
            .map(|(k, v)| Ok((from_redis_value(k)?, from_redis_value(v)?)))
            .collect()
    }
}

#[cfg(feature = "ahash")]
impl<K: FromRedisValue + Eq + Hash, V: FromRedisValue, S: BuildHasher + Default> FromRedisValue
    for ahash::AHashMap<K, V, S>
{
    fn from_redis_value(v: &Value) -> RedisResult<ahash::AHashMap<K, V, S>> {
        v.as_map_iter()
            .ok_or_else(|| invalid_type_error_inner!(v, "Response type not hashmap compatible"))?
            .map(|(k, v)| Ok((from_redis_value(k)?, from_redis_value(v)?)))
            .collect()
    }
}

impl<K: FromRedisValue + Eq + Hash, V: FromRedisValue> FromRedisValue for BTreeMap<K, V>
where
    K: Ord,
{
    fn from_redis_value(v: &Value) -> RedisResult<BTreeMap<K, V>> {
        v.as_map_iter()
            .ok_or_else(|| invalid_type_error_inner!(v, "Response type not btreemap compatible"))?
            .map(|(k, v)| Ok((from_redis_value(k)?, from_redis_value(v)?)))
            .collect()
    }
}

impl<T: FromRedisValue + Eq + Hash, S: BuildHasher + Default> FromRedisValue
    for std::collections::HashSet<T, S>
{
    fn from_redis_value(v: &Value) -> RedisResult<std::collections::HashSet<T, S>> {
        let items = v
            .as_sequence()
            .ok_or_else(|| invalid_type_error_inner!(v, "Response type not hashset compatible"))?;
        items.iter().map(|item| from_redis_value(item)).collect()
    }
}

#[cfg(feature = "ahash")]
impl<T: FromRedisValue + Eq + Hash, S: BuildHasher + Default> FromRedisValue
    for ahash::AHashSet<T, S>
{
    fn from_redis_value(v: &Value) -> RedisResult<ahash::AHashSet<T, S>> {
        let items = v
            .as_sequence()
            .ok_or_else(|| invalid_type_error_inner!(v, "Response type not hashset compatible"))?;
        items.iter().map(|item| from_redis_value(item)).collect()
    }
}

impl<T: FromRedisValue + Eq + Hash> FromRedisValue for BTreeSet<T>
where
    T: Ord,
{
    fn from_redis_value(v: &Value) -> RedisResult<BTreeSet<T>> {
        let items = v
            .as_sequence()
            .ok_or_else(|| invalid_type_error_inner!(v, "Response type not btreeset compatible"))?;
        items.iter().map(|item| from_redis_value(item)).collect()
    }
}

impl FromRedisValue for Value {
    fn from_redis_value(v: &Value) -> RedisResult<Value> {
        Ok(v.clone())
    }
}

impl FromRedisValue for () {
    fn from_redis_value(_v: &Value) -> RedisResult<()> {
        Ok(())
    }
}

macro_rules! from_redis_value_for_tuple {
    () => ();
    ($($name:ident,)+) => (
        #[doc(hidden)]
        impl<$($name: FromRedisValue),*> FromRedisValue for ($($name,)*) {
            // we have local variables named T1 as dummies and those
            // variables are unused.
            #[allow(non_snake_case, unused_variables)]
            fn from_redis_value(v: &Value) -> RedisResult<($($name,)*)> {
                match *v {
                    Value::Bulk(ref items) => {
                        // hacky way to count the tuple size
                        let mut n = 0;
                        $(let $name = (); n += 1;)*
                        if items.len() != n {
                            invalid_type_error!(v, "Bulk response of wrong dimension")
                        }

                        // this is pretty ugly too.  The { i += 1; i - 1} is rust's
                        // postfix increment :)
                        let mut i = 0;
                        Ok(($({let $name = (); from_redis_value(
                             &items[{ i += 1; i - 1 }])?},)*))
                    }
                    _ => invalid_type_error!(v, "Not a bulk response")
                }
            }

            #[allow(non_snake_case, unused_variables)]
            fn from_redis_values(items: &[Value]) -> RedisResult<Vec<($($name,)*)>> {
                // hacky way to count the tuple size
                let mut n = 0;
                $(let $name = (); n += 1;)*
                if items.len() % n != 0 {
                    invalid_type_error!(items, "Bulk response of wrong dimension")
                }

                // this is pretty ugly too.  The { i += 1; i - 1} is rust's
                // postfix increment :)
                let mut rv = vec![];
                if items.len() == 0 {
                    return Ok(rv)
                }
                for chunk in items.chunks_exact(n) {
                    match chunk {
                        [$($name),*] => rv.push(($(from_redis_value($name)?),*),),
                         _ => unreachable!(),
                    }
                }
                Ok(rv)
            }
        }
        from_redis_value_for_tuple_peel!($($name,)*);
    )
}

/// This chips of the leading one and recurses for the rest.  So if the first
/// iteration was T1, T2, T3 it will recurse to T2, T3.  It stops for tuples
/// of size 1 (does not implement down to unit).
macro_rules! from_redis_value_for_tuple_peel {
    ($name:ident, $($other:ident,)*) => (from_redis_value_for_tuple!($($other,)*);)
}

from_redis_value_for_tuple! { T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, }

impl FromRedisValue for InfoDict {
    fn from_redis_value(v: &Value) -> RedisResult<InfoDict> {
        let s: String = from_redis_value(v)?;
        Ok(InfoDict::new(&s))
    }
}

impl<T: FromRedisValue> FromRedisValue for Option<T> {
    fn from_redis_value(v: &Value) -> RedisResult<Option<T>> {
        if *v == Value::Nil {
            return Ok(None);
        }
        Ok(Some(from_redis_value(v)?))
    }
}

#[cfg(feature = "bytes")]
impl FromRedisValue for bytes::Bytes {
    fn from_redis_value(v: &Value) -> RedisResult<Self> {
        match v {
            Value::Data(bytes_vec) => Ok(bytes::Bytes::copy_from_slice(bytes_vec.as_ref())),
            _ => invalid_type_error!(v, "Not binary data"),
        }
    }
}

/// A shortcut function to invoke `FromRedisValue::from_redis_value`
/// to make the API slightly nicer.
pub fn from_redis_value<T: FromRedisValue>(v: &Value) -> RedisResult<T> {
    FromRedisValue::from_redis_value(v)
}
