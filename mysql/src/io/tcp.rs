// Copyright (c) 2020 rust-mysql-simple contributors
//
// Licensed under the Apache License, Version 2.0
// <LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0> or the MIT
// license <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. All files in the project carrying such notice may not be copied,
// modified, or distributed except according to those terms.

use std::{io, net::SocketAddr, time::Duration};

use lunatic::net::{TcpStream, ToSocketAddrs};

pub struct MyTcpBuilder<T> {
    address: T,
    bind_address: Option<SocketAddr>,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    keepalive_time_ms: Option<u32>,
    nodelay: bool,
}

impl<T: ToSocketAddrs> MyTcpBuilder<T> {
    pub fn keepalive_time_ms(&mut self, keepalive_time_ms: Option<u32>) -> &mut Self {
        self.keepalive_time_ms = keepalive_time_ms;
        self
    }

    pub fn nodelay(&mut self, nodelay: bool) -> &mut Self {
        self.nodelay = nodelay;
        self
    }

    pub fn write_timeout(&mut self, write_timeout: Option<Duration>) -> &mut Self {
        self.write_timeout = write_timeout;
        self
    }

    pub fn read_timeout(&mut self, read_timeout: Option<Duration>) -> &mut Self {
        self.read_timeout = read_timeout;
        self
    }

    pub fn bind_address<U>(&mut self, bind_address: Option<U>) -> &mut Self
    where
        U: Into<SocketAddr>,
    {
        self.bind_address = bind_address.map(Into::into);
        self
    }

    pub fn connect_timeout(&mut self, timeout: Option<Duration>) -> &mut Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn new(address: T) -> MyTcpBuilder<T> {
        MyTcpBuilder {
            address,
            bind_address: None,
            connect_timeout: None,
            read_timeout: None,
            write_timeout: None,
            keepalive_time_ms: None,
            nodelay: true,
        }
    }

    pub fn connect(self) -> io::Result<TcpStream> {
        let MyTcpBuilder {
            address,
            bind_address,
            connect_timeout,
            read_timeout,
            write_timeout,
            keepalive_time_ms,
            nodelay,
        } = self;
        let err_msg = if bind_address.is_none() {
            "could not connect to any address"
        } else {
            "could not connect to any address with specified bind address"
        };
        let err = io::Error::new(io::ErrorKind::Other, err_msg);

        let addrs = address.to_socket_addrs()?.collect::<Vec<_>>();

        let socket = if let Some(bind_address) = bind_address {
            let fold_fun = |prev, sock_addr: &SocketAddr| match prev {
                Ok(socket) => Ok(socket),
                Err(_) => Ok(TcpStream::connect(bind_address).unwrap()),
            };

            if bind_address.is_ipv4() {
                // client wants to bind to ipv4, so let's look for ipv4 addresses first
                addrs
                    .iter()
                    .filter(|x| x.is_ipv4())
                    .fold(Err(err), fold_fun)
                    .or_else(|e| addrs.iter().filter(|x| x.is_ipv6()).fold(Err(e), fold_fun))
            } else {
                // client wants to bind to ipv6, so let's look for ipv6 addresses first
                addrs
                    .iter()
                    .filter(|x| x.is_ipv6())
                    .fold(Err(err), fold_fun)
                    .or_else(|e| addrs.iter().filter(|x| x.is_ipv4()).fold(Err(e), fold_fun))
            }
        } else {
            // no bind address
            addrs
                .into_iter()
                .fold(Err(err), |prev, sock_addr| match prev {
                    Ok(socket) => Ok(socket),
                    Err(_) => Ok(TcpStream::connect(sock_addr).unwrap()),
                })
        }?;

        // socket.set_read_timeout(read_timeout)?;
        // socket.set_write_timeout(write_timeout)?;
        // if let Some(duration) = keepalive_time_ms {
        //     let conf =
        //         socket2::TcpKeepalive::new().with_time(Duration::from_millis(duration as u64));
        //     socket.set_tcp_keepalive(&conf)?;
        // }
        // socket.set_nodelay(nodelay)?;
        Ok(TcpStream::from(socket))
    }
}
