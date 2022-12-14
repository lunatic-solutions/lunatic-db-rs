use lunatic::net::{TcpStream, TlsStream, ToSocketAddrs};
use serde;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{self, Read, Write};
use std::ops::DerefMut;
use std::path::PathBuf;
use std::str::{from_utf8, FromStr};
use std::time::Duration;

use crate::cmd::{cmd, pipe, Cmd};
use crate::parser::Parser;
use crate::pipeline::Pipeline;
use crate::pubsub::RedisPubSub;
use crate::ErrorKind;
// use crate::pubsub::PubSub;
use crate::types::{from_redis_value, FromRedisValue, RedisError, RedisResult, ToRedisArgs, Value};

static DEFAULT_PORT: u16 = 6379;

/// This function takes a redis URL string and parses it into a URL
/// as used by rust-url.  This is necessary as the default parser does
/// not understand how redis URLs function.
pub fn parse_redis_url(input: &str) -> Option<url::Url> {
    match url::Url::parse(input) {
        Ok(result) => match result.scheme() {
            "redis" | "rediss" | "redis+unix" | "unix" => Some(result),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Defines the connection address.
///
/// Not all connection addresses are supported on all platforms.  For instance
/// to connect to a unix socket you need to run this on an operating system
/// that supports them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Eq)]
pub enum ConnectionAddr {
    /// Format for this is `(host, port)`.
    Tcp(String, u16),
    /// Format for this is `(host, port)`.
    TcpTls {
        /// Hostname
        host: String,
        /// Port
        port: u16,
        /// Disable hostname verification when connecting.
        ///
        /// # Warning
        ///
        /// You should think very carefully before you use this method. If hostname
        /// verification is not used, any valid certificate for any site will be
        /// trusted for use from any other. This introduces a significant
        /// vulnerability to man-in-the-middle attacks.
        insecure: bool,
    },
    /// Format for this is the path to the unix socket.
    Unix(PathBuf),
}

impl ConnectionAddr {
    /// Checks if this address is supported.
    ///
    /// Because not all platforms support all connection addresses this is a
    /// quick way to figure out if a connection method is supported.  Currently
    /// this only affects unix connections which are only supported on unix
    /// platforms and on older versions of rust also require an explicit feature
    /// to be enabled.
    pub fn is_supported(&self) -> bool {
        match *self {
            ConnectionAddr::Tcp(_, _) => true,
            ConnectionAddr::TcpTls { .. } => true,
            ConnectionAddr::Unix(_) => false,
        }
    }
}

impl fmt::Display for ConnectionAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConnectionAddr::Tcp(ref host, port) => write!(f, "{}:{}", host, port),
            ConnectionAddr::TcpTls { ref host, port, .. } => write!(f, "{}:{}", host, port),
            ConnectionAddr::Unix(ref path) => write!(f, "{}", path.display()),
        }
    }
}

/// Holds the connection information that redis should use for connecting.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// A connection address for where to connect to.
    pub addr: ConnectionAddr,

    /// A boxed connection address for where to connect to.
    pub redis: RedisConnectionInfo,
}

/// Redis specific/connection independent information used to establish a connection to redis.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RedisConnectionInfo {
    /// The database number to use.  This is usually `0`.
    pub db: i64,
    /// Optionally a username that should be used for connection.
    pub username: Option<String>,
    /// Optionally a password that should be used for connection.
    pub password: Option<String>,
}

impl FromStr for ConnectionInfo {
    type Err = RedisError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.into_connection_info()
    }
}

/// Converts an object into a connection info struct.  This allows the
/// constructor of the client to accept connection information in a
/// range of different formats.
pub trait IntoConnectionInfo {
    /// Converts the object into a connection info object.
    fn into_connection_info(self) -> RedisResult<ConnectionInfo>;
}

impl IntoConnectionInfo for ConnectionInfo {
    fn into_connection_info(self) -> RedisResult<ConnectionInfo> {
        Ok(self)
    }
}

impl<'a> IntoConnectionInfo for &'a str {
    fn into_connection_info(self) -> RedisResult<ConnectionInfo> {
        match parse_redis_url(self) {
            Some(u) => u.into_connection_info(),
            None => fail!((ErrorKind::InvalidClientConfig, "Redis URL did not parse")),
        }
    }
}

impl<T> IntoConnectionInfo for (T, u16)
where
    T: Into<String>,
{
    fn into_connection_info(self) -> RedisResult<ConnectionInfo> {
        Ok(ConnectionInfo {
            addr: ConnectionAddr::Tcp(self.0.into(), self.1),
            redis: RedisConnectionInfo::default(),
        })
    }
}

impl IntoConnectionInfo for String {
    fn into_connection_info(self) -> RedisResult<ConnectionInfo> {
        match parse_redis_url(&self) {
            Some(u) => u.into_connection_info(),
            None => fail!((ErrorKind::InvalidClientConfig, "Redis URL did not parse")),
        }
    }
}

fn url_to_tcp_connection_info(url: url::Url) -> RedisResult<ConnectionInfo> {
    let host = match url.host() {
        Some(host) => host.to_string(),
        None => fail!((ErrorKind::InvalidClientConfig, "Missing hostname")),
    };
    let port = url.port().unwrap_or(DEFAULT_PORT);
    let addr = if url.scheme() == "rediss" {
        match url.fragment() {
            Some("insecure") => ConnectionAddr::TcpTls {
                host,
                port,
                insecure: true,
            },
            Some(_) => fail!((
                ErrorKind::InvalidClientConfig,
                "only #insecure is supported as URL fragment"
            )),
            _ => ConnectionAddr::TcpTls {
                host,
                port,
                insecure: false,
            },
        }
    } else {
        ConnectionAddr::Tcp(host, port)
    };
    Ok(ConnectionInfo {
        addr,
        redis: RedisConnectionInfo {
            db: match url.path().trim_matches('/') {
                "" => 0,
                path => unwrap_or!(
                    path.parse::<i64>().ok(),
                    fail!((ErrorKind::InvalidClientConfig, "Invalid database number"))
                ),
            },
            username: if url.username().is_empty() {
                None
            } else {
                match percent_encoding::percent_decode(url.username().as_bytes()).decode_utf8() {
                    Ok(decoded) => Some(decoded.into_owned()),
                    Err(_) => fail!((
                        ErrorKind::InvalidClientConfig,
                        "Username is not valid UTF-8 string"
                    )),
                }
            },
            password: match url.password() {
                Some(pw) => match percent_encoding::percent_decode(pw.as_bytes()).decode_utf8() {
                    Ok(decoded) => Some(decoded.into_owned()),
                    Err(_) => fail!((
                        ErrorKind::InvalidClientConfig,
                        "Password is not valid UTF-8 string"
                    )),
                },
                None => None,
            },
        },
    })
}

#[cfg(not(unix))]
fn url_to_unix_connection_info(_: url::Url) -> RedisResult<ConnectionInfo> {
    fail!((
        ErrorKind::InvalidClientConfig,
        "Unix sockets are not available on this platform."
    ));
}

impl IntoConnectionInfo for url::Url {
    fn into_connection_info(self) -> RedisResult<ConnectionInfo> {
        match self.scheme() {
            "redis" | "rediss" => url_to_tcp_connection_info(self),
            "unix" | "redis+unix" => url_to_unix_connection_info(self),
            _ => fail!((
                ErrorKind::InvalidClientConfig,
                "URL provided is not a redis URL"
            )),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct TcpConnection {
    pub(crate) reader: TcpStream,
    open: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TcpTlsConnection {
    reader: TlsStream,
    open: bool,
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) enum ActualConnection {
    Tcp(TcpConnection),
    TcpTls(TcpTlsConnection),
}

/// Represents a stateful redis TCP connection.
#[derive(Serialize, Deserialize)]
pub struct Connection {
    pub(crate) con: ActualConnection,
    #[serde(skip_serializing, skip_deserializing)]
    parser: Parser,
    db: i64,

    /// Flag indicating whether the connection was left in the PubSub state after dropping `PubSub`.
    ///
    /// This flag is checked when attempting to send a command, and if it's raised, we attempt to
    /// exit the pubsub state before executing the new request.
    pubsub: bool,
}

/// Represents a stateful redis TCP connection that can be moved to separate processes.
#[derive(Deserialize, Serialize, Clone)]
pub struct StrippedConnection {
    pub(crate) con: ActualConnection,
    db: i64,

    /// Flag indicating whether the connection was left in the PubSub state after dropping `PubSub`.
    ///
    /// This flag is checked when attempting to send a command, and if it's raised, we attempt to
    /// exit the pubsub state before executing the new request.
    pubsub: bool,
}

impl StrippedConnection {
    pub fn with_parser(&self) -> Connection {
        Connection {
            con: self.con.clone(),
            parser: Parser::new(),
            db: self.db,
            pubsub: self.pubsub,
        }
    }
}

/// Represents a pubsub message.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Msg {
    payload: Value,
    channel: Value,
    pattern: Option<Value>,
}

impl ActualConnection {
    pub fn new(addr: &ConnectionAddr, timeout: Option<Duration>) -> RedisResult<ActualConnection> {
        Ok(match *addr {
            ConnectionAddr::Tcp(ref host, ref port) => {
                let host: &str = &**host;
                let tcp = match timeout {
                    None => TcpStream::connect(format!("{}:{}", host, *port))?,
                    Some(timeout) => {
                        let mut tcp = None;
                        let mut last_error = None;
                        for addr in format!("{}:{}", host, *port).to_socket_addrs()? {
                            match TcpStream::connect_timeout(addr, timeout) {
                                Ok(l) => {
                                    tcp = Some(l);
                                    break;
                                }
                                Err(e) => {
                                    last_error = Some(e);
                                }
                            };
                        }
                        match (tcp, last_error) {
                            (Some(tcp), _) => tcp,
                            (None, Some(e)) => {
                                fail!(e);
                            }
                            (None, None) => {
                                fail!((
                                    ErrorKind::InvalidClientConfig,
                                    "could not resolve to any addresses"
                                ));
                            }
                        }
                    }
                };
                ActualConnection::Tcp(TcpConnection {
                    reader: tcp,
                    open: true,
                })
            }
            ConnectionAddr::TcpTls { ref host, port, .. } => {
                let tls = match timeout {
                    None => match TlsStream::connect(host, port.into()) {
                        Ok(res) => res,
                        Err(e) => {
                            fail!((ErrorKind::IoError, "SSL Handshake error", e.to_string()));
                        }
                    },
                    Some(timeout) => {
                        TlsStream::connect_timeout(host, timeout, port.into(), vec![]).unwrap()
                    }
                };
                ActualConnection::TcpTls(TcpTlsConnection {
                    reader: tls,
                    open: true,
                })
            }
            #[cfg(not(unix))]
            ConnectionAddr::Unix(ref _path) => {
                fail!((
                    ErrorKind::InvalidClientConfig,
                    "Cannot connect to unix sockets \
                     on this platform"
                ));
            }
        })
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> RedisResult<Value> {
        match *self {
            ActualConnection::Tcp(ref mut connection) => {
                let res = connection.reader.write_all(bytes).map_err(RedisError::from);
                match res {
                    Err(e) => {
                        if e.is_connection_dropped() {
                            connection.open = false;
                        }
                        Err(e)
                    }
                    Ok(_) => Ok(Value::Okay),
                }
            }
            ActualConnection::TcpTls(ref mut connection) => {
                let res = connection.reader.write_all(bytes).map_err(RedisError::from);
                match res {
                    Err(e) => {
                        if e.is_connection_dropped() {
                            connection.open = false;
                        }
                        Err(e)
                    }
                    Ok(_) => Ok(Value::Okay),
                }
            }
        }
    }

    pub fn set_write_timeout(&mut self, dur: Option<Duration>) -> RedisResult<()> {
        match self {
            ActualConnection::Tcp(conn) => {
                conn.reader.set_write_timeout(dur)?;
            }
            ActualConnection::TcpTls(TcpTlsConnection { ref mut reader, .. }) => {
                reader.set_write_timeout(dur)?;
            }
        }
        Ok(())
    }

    pub fn set_read_timeout(&mut self, dur: Option<Duration>) -> RedisResult<()> {
        match self {
            ActualConnection::Tcp(conn) => {
                conn.reader.set_read_timeout(dur)?;
            }
            ActualConnection::TcpTls(TcpTlsConnection { ref mut reader, .. }) => {
                reader.set_read_timeout(dur)?;
            }
        }
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        match *self {
            ActualConnection::Tcp(TcpConnection { open, .. }) => open,
            ActualConnection::TcpTls(TcpTlsConnection { open, .. }) => open,
        }
    }
}

fn connect_auth(con: &mut Connection, connection_info: &RedisConnectionInfo) -> RedisResult<()> {
    let mut command = cmd("AUTH");
    if let Some(username) = &connection_info.username {
        command.arg(username);
    }
    let password = connection_info.password.as_ref().unwrap();
    let err = match command.arg(password).query::<Value>(con) {
        Ok(Value::Okay) => return Ok(()),
        Ok(_) => {
            fail!((
                ErrorKind::ResponseError,
                "Redis server refused to authenticate, returns Ok() != Value::Okay"
            ));
        }
        Err(e) => e,
    };
    let err_msg = err.detail().ok_or((
        ErrorKind::AuthenticationFailed,
        "Password authentication failed",
    ))?;
    if !err_msg.contains("wrong number of arguments for 'auth' command") {
        fail!((
            ErrorKind::AuthenticationFailed,
            "Password authentication failed",
        ));
    }

    // fallback to AUTH version <= 5
    let mut command = cmd("AUTH");
    match command.arg(password).query::<Value>(con) {
        Ok(Value::Okay) => Ok(()),
        _ => fail!((
            ErrorKind::AuthenticationFailed,
            "Password authentication failed",
        )),
    }
}

pub fn connect(
    connection_info: &ConnectionInfo,
    timeout: Option<Duration>,
) -> RedisResult<Connection> {
    let con = ActualConnection::new(&connection_info.addr, timeout)?;
    setup_connection(con, &connection_info.redis)
}

fn setup_connection(
    con: ActualConnection,
    connection_info: &RedisConnectionInfo,
) -> RedisResult<Connection> {
    let mut rv = Connection {
        con,
        parser: Parser::new(),
        db: connection_info.db,
        pubsub: false,
    };

    if connection_info.password.is_some() {
        connect_auth(&mut rv, connection_info)?;
    }

    if connection_info.db != 0 {
        match cmd("SELECT")
            .arg(connection_info.db)
            .query::<Value>(&mut rv)
        {
            Ok(Value::Okay) => {}
            _ => fail!((
                ErrorKind::ResponseError,
                "Redis server refused to switch database"
            )),
        }
    }

    Ok(rv)
}

/// Implements the "stateless" part of the connection interface that is used by the
/// different objects in lunatic_redis.  Primarily it obviously applies to `Connection`
/// object but also some other objects implement the interface (for instance
/// whole clients or certain redis results).
///
/// Generally clients and connections (as well as redis results of those) implement
/// this trait.  Actual connections provide more functionality which can be used
/// to implement things like `PubSub` but they also can modify the intrinsic
/// state of the TCP connection.  This is not possible with `ConnectionLike`
/// implementors because that functionality is not exposed.
pub trait ConnectionLike {
    /// Sends an already encoded (packed) command into the TCP socket and
    /// reads the single response from it.
    fn req_packed_command(&mut self, cmd: &[u8]) -> RedisResult<Value>;

    /// Sends multiple already encoded (packed) command into the TCP socket
    /// and reads `count` responses from it.  This is used to implement
    /// pipelining.
    fn req_packed_commands(
        &mut self,
        cmd: &[u8],
        offset: usize,
        count: usize,
    ) -> RedisResult<Vec<Value>>;

    /// Sends a [Cmd](Cmd) into the TCP socket and reads a single response from it.
    fn req_command(&mut self, cmd: &Cmd) -> RedisResult<Value> {
        let pcmd = cmd.get_packed_command();
        self.req_packed_command(&pcmd)
    }

    /// Returns the database this connection is bound to.  Note that this
    /// information might be unreliable because it's initially cached and
    /// also might be incorrect if the connection like object is not
    /// actually connected.
    fn get_db(&self) -> i64;

    /// Does this connection support pipelining?
    #[doc(hidden)]
    fn supports_pipelining(&self) -> bool {
        true
    }

    /// Check that all connections it has are available (`PING` internally).
    fn check_connection(&mut self) -> bool;

    /// Returns the connection status.
    ///
    /// The connection is open until any `read_response` call recieved an
    /// invalid response from the server (most likely a closed or dropped
    /// connection, otherwise a Redis protocol error). When using unix
    /// sockets the connection is open until writing a command failed with a
    /// `BrokenPipe` error.
    fn is_open(&self) -> bool;
}

impl Clone for Connection {
    fn clone(&self) -> Self {
        Self {
            con: self.con.clone(),
            pubsub: self.pubsub,
            db: self.db,
            parser: Parser::new(),
        }
    }
}

/// A connection is an object that represents a single redis connection.  It
/// provides basic support for sending encoded commands into a redis connection
/// and to read a response from it.  It's bound to a single database and can
/// only be created from the client.
///
/// You generally do not much with this object other than passing it to
/// `Cmd` objects.
impl Connection {
    /// strips the connection of the parser so that it can copied over to other processes
    pub fn strip(&self) -> StrippedConnection {
        StrippedConnection {
            con: self.con.clone(),
            db: self.db,
            pubsub: self.pubsub,
        }
    }

    /// Sends an already encoded (packed) command into the TCP socket and
    /// does not read a response.  This is useful for commands like
    /// `MONITOR` which yield multiple items.  This needs to be used with
    /// care because it changes the state of the connection.
    pub fn send_packed_command(&mut self, cmd: &[u8]) -> RedisResult<()> {
        self.con.send_bytes(cmd)?;
        Ok(())
    }

    /// Fetches a single response from the connection.  This is useful
    /// if used in combination with `send_packed_command`.
    pub fn recv_response<T: Read>(&mut self) -> RedisResult<Value> {
        self.read_response(None as Option<&mut T>)
    }

    /// Sets the write timeout for the connection.
    ///
    /// If the provided value is `None`, then `send_packed_command` call will
    /// block indefinitely. It is an error to pass the zero `Duration` to this
    /// method.
    pub fn set_write_timeout(&mut self, dur: Option<Duration>) -> RedisResult<()> {
        self.con.set_write_timeout(dur)
    }

    /// Sets the read timeout for the connection.
    ///
    /// If the provided value is `None`, then `recv_response` call will
    /// block indefinitely. It is an error to pass the zero `Duration` to this
    /// method.
    pub fn set_read_timeout(&mut self, dur: Option<Duration>) -> RedisResult<()> {
        self.con.set_read_timeout(dur)
    }

    /// Creates a [`RedisPubSub`] instance for this connection.
    /// this moves the connection so that there's no accidental usage of the connection
    /// besides via the subscription interface
    pub fn as_pubsub(self) -> RedisPubSub {
        // NOTE: The pubsub flag is intentionally not raised at this time since
        // running commands within the pubsub state should not try and exit from
        // the pubsub state.
        RedisPubSub::new(self)
    }
    /// Fetches a single response from the connection.
    fn read_response<T: Read>(&mut self, reader: Option<&mut T>) -> RedisResult<Value> {
        let result = match (reader, &mut self.con) {
            (Some(reader), _) => self.parser.parse_value(reader),
            (None, ActualConnection::Tcp(TcpConnection { reader, .. })) => {
                self.parser.parse_value(reader)
            }
            (None, ActualConnection::TcpTls(TcpTlsConnection { ref mut reader, .. })) => {
                self.parser.parse_value(reader)
            }
        };
        // shutdown connection on protocol error
        if let Err(e) = &result {
            let shutdown = match e.as_io_error() {
                Some(e) => e.kind() == io::ErrorKind::UnexpectedEof,
                None => false,
            };
            if shutdown {
                match self.con {
                    ActualConnection::Tcp(ref mut _connection) => {
                        // let _ = connection.reader.shutdown(net::Shutdown::Both);
                        // connection.reader.connection.open = false;
                    }
                    ActualConnection::TcpTls(ref mut _connection) => {
                        // let _ = connection.reader.shutdown();
                        // connection.open = false;
                    }
                }
            }
        }
        result
    }
}

impl ConnectionLike for Connection {
    fn req_packed_command(&mut self, cmd: &[u8]) -> RedisResult<Value> {
        // if self.pubsub {
        //     self.exit_pubsub()?;
        // }

        self.con.send_bytes(cmd)?;
        self.read_response::<TcpStream>(None)
    }

    fn req_packed_commands(
        &mut self,
        cmd: &[u8],
        offset: usize,
        count: usize,
    ) -> RedisResult<Vec<Value>> {
        // if self.pubsub {
        //     self.exit_pubsub()?;
        // }
        self.con.send_bytes(cmd)?;
        let mut rv = vec![];
        let mut first_err = None;
        for idx in 0..(offset + count) {
            // When processing a transaction, some responses may be errors.
            // We need to keep processing the rest of the responses in that case,
            // so bailing early with `?` would not be correct.
            let response = self.read_response(None as Option<&mut TcpStream>);
            match response {
                Ok(item) => {
                    if idx >= offset {
                        rv.push(item);
                    }
                }
                Err(err) => {
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
            }
        }

        first_err.map_or(Ok(rv), Err)
    }

    fn get_db(&self) -> i64 {
        self.db
    }

    fn is_open(&self) -> bool {
        self.con.is_open()
    }

    fn check_connection(&mut self) -> bool {
        cmd("PING").query::<String>(self).is_ok()
    }
}

impl<C, T> ConnectionLike for T
where
    C: ConnectionLike,
    T: DerefMut<Target = C>,
{
    fn req_packed_command(&mut self, cmd: &[u8]) -> RedisResult<Value> {
        self.deref_mut().req_packed_command(cmd)
    }

    fn req_packed_commands(
        &mut self,
        cmd: &[u8],
        offset: usize,
        count: usize,
    ) -> RedisResult<Vec<Value>> {
        self.deref_mut().req_packed_commands(cmd, offset, count)
    }

    fn req_command(&mut self, cmd: &Cmd) -> RedisResult<Value> {
        self.deref_mut().req_command(cmd)
    }

    fn get_db(&self) -> i64 {
        self.deref().get_db()
    }

    fn supports_pipelining(&self) -> bool {
        self.deref().supports_pipelining()
    }

    fn check_connection(&mut self) -> bool {
        self.deref_mut().check_connection()
    }

    fn is_open(&self) -> bool {
        self.deref().is_open()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) enum Confirmation {
    Pattern(String),
    Punsub(String),
    Topic(String),
    Unsub(String),
}

impl Confirmation {
    pub(crate) fn check_confirmation(value: &Value) -> Option<Self> {
        let raw_msg: Vec<Value> = from_redis_value(value).ok()?;
        let mut iter = raw_msg.iter();
        let msg_type: String = from_redis_value(iter.next()?).ok()?;
        let msg_type = msg_type.as_str();
        if !["unsubscribe", "punsubscribe", "subscribe", "psubscribe"].contains(&msg_type) {
            return None;
        }
        // start iterator to actually skip
        let matching: String = from_redis_value(iter.next()?).ok()?;
        Some(match msg_type {
            "psubscribe" => Confirmation::Pattern(matching),
            "subscribe" => Confirmation::Topic(matching),
            "punsubscribe" => Confirmation::Punsub(matching),
            "unsubscribe" => Confirmation::Unsub(matching),
            _ => unreachable!(),
        })
    }
}

/// This holds the data that comes from listening to a pubsub
/// connection.  It only contains actual message data.
impl Msg {
    /// Tries to convert provided [`Value`] into [`Msg`].
    pub fn from_value(value: &Value) -> Option<Self> {
        let raw_msg: Vec<Value> = from_redis_value(value).ok()?;
        let mut iter = raw_msg.into_iter();
        let msg_type: String = from_redis_value(&iter.next()?).ok()?;
        let mut pattern = None;
        let payload;
        let channel;

        if msg_type == "message" {
            channel = iter.next()?;
            payload = iter.next()?;
        } else if msg_type == "pmessage" {
            pattern = Some(iter.next()?);
            channel = iter.next()?;
            payload = iter.next()?;
        } else {
            return None;
        }

        Some(Msg {
            payload,
            channel,
            pattern,
        })
    }

    /// Returns the channel this message came on.
    pub fn get_channel<T: FromRedisValue>(&self) -> RedisResult<T> {
        from_redis_value(&self.channel)
    }

    /// Convenience method to get a string version of the channel.  Unless
    /// your channel contains non utf-8 bytes you can always use this
    /// method.  If the channel is not a valid string (which really should
    /// not happen) then the return value is `"?"`.
    pub fn get_channel_name(&self) -> &str {
        match self.channel {
            Value::Data(ref bytes) => from_utf8(bytes).unwrap_or("?"),
            _ => "?",
        }
    }

    /// Returns the message's payload in a specific format.
    pub fn get_payload<T: FromRedisValue>(&self) -> RedisResult<T> {
        from_redis_value(&self.payload)
    }

    /// Returns the bytes that are the message's payload.  This can be used
    /// as an alternative to the `get_payload` function if you are interested
    /// in the raw bytes in it.
    pub fn get_payload_bytes(&self) -> &[u8] {
        match self.payload {
            Value::Data(ref bytes) => bytes,
            _ => b"",
        }
    }

    /// Returns true if the message was constructed from a pattern
    /// subscription.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_pattern(&self) -> bool {
        self.pattern.is_some()
    }

    /// If the message was constructed from a message pattern this can be
    /// used to find out which one.  It's recommended to match against
    /// an `Option<String>` so that you do not need to use `from_pattern`
    /// to figure out if a pattern was set.
    pub fn get_pattern<T: FromRedisValue>(&self) -> RedisResult<T> {
        match self.pattern {
            None => from_redis_value(&Value::Nil),
            Some(ref x) => from_redis_value(x),
        }
    }
}

/// This function simplifies transaction management slightly.  What it
/// does is automatically watching keys and then going into a transaction
/// loop util it succeeds.  Once it goes through the results are
/// returned.
///
/// To use the transaction two pieces of information are needed: a list
/// of all the keys that need to be watched for modifications and a
/// closure with the code that should be execute in the context of the
/// transaction.  The closure is invoked with a fresh pipeline in atomic
/// mode.  To use the transaction the function needs to return the result
/// from querying the pipeline with the connection.
///
/// The end result of the transaction is then available as the return
/// value from the function call.
///
/// Example:
///
/// ```rust,no_run
/// use redis::Commands;
/// # fn do_something() -> redis::RedisResult<()> {
/// # let client = redis::Client::open("redis://127.0.0.1/").unwrap();
/// # let mut con = client.get_connection().unwrap();
/// let key = "the_key";
/// let (new_val,) : (isize,) = redis::transaction(&mut con, &[key], |con, pipe| {
///     let old_val : isize = con.get(key)?;
///     pipe
///         .set(key, old_val + 1).ignore()
///         .get(key).query(con)
/// })?;
/// println!("The incremented number is: {}", new_val);
/// # Ok(()) }
/// ```
pub fn transaction<
    C: ConnectionLike,
    K: ToRedisArgs,
    T,
    F: FnMut(&mut C, &mut Pipeline) -> RedisResult<Option<T>>,
>(
    con: &mut C,
    keys: &[K],
    func: F,
) -> RedisResult<T> {
    let mut func = func;
    loop {
        cmd("WATCH").arg(keys).query::<()>(con)?;
        let mut p = pipe();
        let response: Option<T> = func(con, p.atomic())?;
        match response {
            None => {
                continue;
            }
            Some(response) => {
                // make sure no watch is left in the connection, even if
                // someone forgot to use the pipeline.
                cmd("UNWATCH").query::<()>(con)?;
                return Ok(response);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_redis_url() {
        let cases = vec![
            ("redis://127.0.0.1", true),
            ("http://127.0.0.1", false),
            ("tcp://127.0.0.1", false),
        ];
        for (url, expected) in cases.into_iter() {
            let res = parse_redis_url(url);
            assert_eq!(
                res.is_some(),
                expected,
                "Parsed result of `{}` is not expected",
                url,
            );
        }
    }

    #[test]
    fn test_url_to_tcp_connection_info() {
        let cases = vec![
            (
                url::Url::parse("redis://127.0.0.1").unwrap(),
                ConnectionInfo {
                    addr: ConnectionAddr::Tcp("127.0.0.1".to_string(), 6379),
                    redis: Default::default(),
                },
            ),
            (
                url::Url::parse("redis://%25johndoe%25:%23%40%3C%3E%24@example.com/2").unwrap(),
                ConnectionInfo {
                    addr: ConnectionAddr::Tcp("example.com".to_string(), 6379),
                    redis: RedisConnectionInfo {
                        db: 2,
                        username: Some("%johndoe%".to_string()),
                        password: Some("#@<>$".to_string()),
                    },
                },
            ),
        ];
        for (url, expected) in cases.into_iter() {
            let res = url_to_tcp_connection_info(url.clone()).unwrap();
            assert_eq!(res.addr, expected.addr, "addr of {} is not expected", url);
            assert_eq!(
                res.redis.db, expected.redis.db,
                "db of {} is not expected",
                url
            );
            assert_eq!(
                res.redis.username, expected.redis.username,
                "username of {} is not expected",
                url
            );
            assert_eq!(
                res.redis.password, expected.redis.password,
                "password of {} is not expected",
                url
            );
        }
    }

    #[test]
    fn test_url_to_tcp_connection_info_failed() {
        let cases = vec![
            (url::Url::parse("redis://").unwrap(), "Missing hostname"),
            (
                url::Url::parse("redis://127.0.0.1/db").unwrap(),
                "Invalid database number",
            ),
            (
                url::Url::parse("redis://C3%B0@127.0.0.1").unwrap(),
                "Username is not valid UTF-8 string",
            ),
            (
                url::Url::parse("redis://:C3%B0@127.0.0.1").unwrap(),
                "Password is not valid UTF-8 string",
            ),
        ];
        for (url, expected) in cases.into_iter() {
            let res = url_to_tcp_connection_info(url);
            assert_eq!(
                res.as_ref().unwrap_err().kind(),
                crate::ErrorKind::InvalidClientConfig,
                "{}",
                res.as_ref().unwrap_err(),
            );
            assert_eq!(
                res.as_ref().unwrap_err().to_string(),
                expected,
                "{}",
                res.as_ref().unwrap_err(),
            );
        }
    }
}
