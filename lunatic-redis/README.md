# lunatic_redis

[![Rust](https://github.com/lunatic-solutions/lunatic-db/lunatic-redis/actions/workflows/rust.yml/badge.svg)](https://github.com/lunatic-solutions/lunatic-db/lunatic-redis/actions/workflows/rust.yml)
[![crates.io](https://img.shields.io/crates/v/redis.svg)](https://crates.io/crates/lunatic_redis)
[![Chat](https://img.shields.io/discord/976380008299917365?logo=discord)](https://discord.gg/UNNeswQS)

Lunatic-Redis-rs is a high level redis library for the lunatic VM for Rust.
It provides convenient access
to all Redis functionality through a very flexible but low-level API.  It
uses a customizable type conversion trait so that any operation can return
results in just the type you are expecting.  This makes for a very pleasant
development experience.

The crate is called `lunatic_redis` and you can depend on it via cargo:

```ini
[dependencies]
lunatic_redis = "0.1.0"
```

Documentation on the library can be found at
[docs.rs/lunatic_redis](https://docs.rs/lunatic_redis).

**Note: lunatic_redis requires at least Lunatic VM 0.12.0 and Rust 1.59.**

## Basic Operation

To open a connection you need to create a client and then to fetch a
connection from it.  In the future there will be a connection pool for
those, currently each connection is separate and not pooled.

Many commands are implemented through the `Commands` trait but manual
command creation is also possible.

```rust
extern crate lunatic_redis;
use lunatic_redis::{self as redis, Commands};

fn fetch_an_integer() -> redis::RedisResult<isize> {
    // connect to redis
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    // throw away the result, just make sure it does not fail
    let _ : () = con.set("my_key", 42)?;
    // read back the key and return it.  Because the return value
    // from the function is a result for integer this will automatically
    // convert into one.
    con.get("my_key")
}
```
## TLS Support

TlS is enabled by default via the lunatic vm
so you should be able to connect to a redis instance using the `rediss://` URL scheme:

```rust
let client = lunatic_redis::Client::open("rediss://127.0.0.1/")?;
```

## Cluster Support

*Currently not supported*
Cluster mode can be used by specifying "cluster" as a features entry in your Cargo.toml.

`redis = { version = "0.22.1", features = [ "cluster"] }`

Then you can simply use the `ClusterClient` which accepts a list of available nodes.

```rust
use redis::cluster::ClusterClient;
use redis::Commands;

fn fetch_an_integer() -> String {
    // connect to redis
    let nodes = vec!["redis://127.0.0.1/"];
    let client = ClusterClient::open(nodes).unwrap();
    let mut connection = client.get_connection().unwrap();
    let _: () = connection.set("test", "test_data").unwrap();
    let rv: String = connection.get("test").unwrap();
    return rv;
}
```

## JSON Support

*Currently not supported*
Support for the RedisJSON Module can be enabled by specifying "json" as a feature in your Cargo.toml.

`redis = { version = "0.22.1", features = ["json"] }`

Then you can simply import the `JsonCommands` trait which will add the `json` commands to all Redis Connections (not to be confused with just `Commands` which only adds the default commands)

```rust
use redis::Client;
use redis::JsonCommands;
use redis::RedisResult;
use redis::ToRedisArgs;

// Result returns Ok(true) if the value was set
// Result returns Err(e) if there was an error with the server itself OR serde_json was unable to serialize the boolean
fn set_json_bool<P: ToRedisArgs>(key: P, path: P, b: bool) -> RedisResult<bool> {
    let client = Client::open("redis://127.0.0.1").unwrap();
    let connection = client.get_connection().unwrap();

    // runs `JSON.SET {key} {path} {b}`
    connection.json_set(key, path, b)?
    
    // you'll need to use serde_json (or some other json lib) to deserialize the results from the bytes
    // It will always be a Vec, if no results were found at the path it'll be an empty Vec
}

```

## Development

To test `redis` you're going to need to be able to test with the Redis Modules, to do this
you must set the following envornment variables before running the test script

- `REDIS_RS_REDIS_JSON_PATH` = The absolute path to the RedisJSON module (Usually called `librejson.so`).

<!-- As support for modules are added later, it would be wise to update this list -->

If you want to develop on the library there are a few commands provided
by the makefile:

To build:

    $ make

To test:

    $ make test

To run benchmarks:

    $ make bench

To build the docs (require nightly compiler, see [rust-lang/rust#43781](https://github.com/rust-lang/rust/issues/43781)):

    $ make docs

We encourage you to run `clippy` prior to seeking a merge for your work.  The lints can be quite strict.  Running this on your own workstation can save you time, since Travis CI will fail any build that doesn't satisfy `clippy`:

    $ cargo clippy --all-features --all --tests --examples -- -D clippy::all -D warnings