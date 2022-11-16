# lunatic-db

A collection of Rust db drivers for [lunatic](https://github.com/lunatic-solutions/lunatic-rs).

The following db drivers are available at the moment:
- [x] MySQL
- [x] Redis


------------------

### **Redis client**

At the moment there's a fairly well working redis driver which is largely based on https://github.com/redis-rs/redis-rs with working examples for the following features:

- [x] Various set/get behaviour
- [x] Geospatial functions
- [x] Pub/Sub functionality
- [x] Queues support
- [x] Streams support
- [x] Familiar API surface as in redis-rs
- [ ] Tls support (not tested because I don't have a TLS redis instance setup yet)

> You can find the redis examples under `lunatic-redis/examples` and there are commands to run them in the root `./Cargo.toml`

In order to make the driver more robust there are still some things to do, namely:
- [ ] make all tests work (requires a rewrite of test utilities)
- [ ] Add automatic reconnections to clients
- [ ] Make sure multiplexing works reliably (e.g. same client used from multiple lunatic processes)
- [ ] Add cluster support
- [ ] Provide an idiomatic pubsub API for lunatic abstractions (not yet settled on how exactly it should look like)



------------------

### **MySQL client**

The MySQL client is less tested at the moment, partially because the original crate also doesn't provide that many examples.
The code is largely based on https://github.com/blackbeam/rust-mysql-simple.git and has some internal connection-related details changed in order to work on the lunatic VM.

There is an example of the client working at `examples/mysql.rs`

The next steps for MySQL are the following:
- [ ] more extensive testing of the library
- [ ] TLS support