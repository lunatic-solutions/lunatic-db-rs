use crate::{cmd::cmd, connection::Confirmation};
use lunatic::{abstract_process, net::TcpStream, process::ProcessRef};
use serde::{Deserialize, Serialize};

use crate::{from_redis_value, Connection, ErrorKind, Msg, RedisError, RedisResult, ToRedisArgs};

/// RedisPubSub allows one to use a connection for pub-sub to publish or subscribe to certain
/// topics and/or patterns.
#[derive(Clone, Deserialize, Serialize)]
pub struct RedisPubSub {
    connection: Connection,
    // are used for restarting connection if redis server resets connection
    subscribed_topics: Vec<String>,
    subscribed_patterns: Vec<String>,
}

#[abstract_process]
impl RedisPubSub {
    #[init]
    fn init(_this: ProcessRef<RedisPubSub>, connection: Connection) -> RedisPubSub {
        RedisPubSub::new(connection)
    }

    // State management functions
    /// create new PubSub connection from regular connection
    pub fn new(connection: Connection) -> Self {
        RedisPubSub {
            connection,
            subscribed_topics: vec![],
            subscribed_patterns: vec![],
        }
    }

    /// Subscribe to a topic. Now the `receive()` function will get messages
    /// on this new topic
    pub fn subscribe<T>(&mut self, topic: T) -> RedisResult<()>
    where
        T: ToRedisArgs + ToString,
    {
        let s = topic.to_string();
        match cmd("SUBSCRIBE")
            .arg(topic)
            .query::<()>(&mut self.connection)
        {
            Err(e) => Err(e),
            Ok(_) => {
                self.subscribed_topics.push(s);
                Ok(())
            }
        }
    }

    /// Subscribe to topics of a certain pattern. Now the `receive()` function
    /// will get messages on topics that match this new pattern
    pub fn psubscribe<T>(&mut self, pattern: T) -> RedisResult<()>
    where
        T: ToRedisArgs + ToString,
    {
        let s = pattern.to_string();
        match cmd("PSUBSCRIBE")
            .arg(pattern)
            .query::<()>(&mut self.connection)
        {
            Err(e) => Err(e),
            Ok(_) => {
                self.subscribed_patterns.push(s);
                Ok(())
            }
        }
    }

    /// Unsubscribe from a topic. `receive()` will not get any more
    /// messages on this topic
    pub fn unsubscribe<T>(&mut self, topic: T) -> RedisResult<()>
    where
        T: ToRedisArgs + ToString,
    {
        let s = topic.to_string();
        match cmd("UNSUBSCRIBE")
            .arg(topic)
            .query::<()>(&mut self.connection)
        {
            Err(e) => Err(e),
            Ok(_) => {
                self.subscribed_topics.retain(|t| *t != s);
                Ok(())
            }
        }
    }

    /// Unsubscribe from topics matching a pattern.
    /// `receive()` will not get any more messages on this topic
    pub fn punsubscribe<T>(&mut self, pattern: T) -> RedisResult<()>
    where
        T: ToRedisArgs + ToString,
    {
        let s = pattern.to_string();
        match cmd("PUNSUBSCRIBE")
            .arg(pattern)
            .query::<()>(&mut self.connection)
        {
            Err(e) => Err(e),
            Ok(_) => {
                self.subscribed_patterns.retain(|t| *t != s);
                Ok(())
            }
        }
    }

    /// clear subscriptions and exit pubsub
    pub fn exit_pubsub(mut self) -> RedisResult<Connection> {
        self.clear_active_subscriptions()?;
        Ok(self.connection)
    }

    /// Get the inner connection out of a PubSub
    ///
    /// Any active subscriptions are unsubscribed. In the event of an error, the connection is
    /// dropped.
    fn clear_active_subscriptions(&mut self) -> RedisResult<()> {
        // Responses to unsubscribe commands return in a 3-tuple with values
        // ("unsubscribe" or "punsubscribe", name of subscription removed, count of remaining subs).
        // The "count of remaining subs" includes both pattern subscriptions and non pattern
        // subscriptions. Thus, to accurately drain all unsubscribe messages received from the
        // server, both commands need to be executed at once.

        // Grab a reference to the underlying connection so that we may send
        // the commands without immediately blocking for a response.
        let connection = &mut self.connection;
        {
            // Prepare both unsubscribe commands
            let unsubscribe = cmd("UNSUBSCRIBE").get_packed_command();
            let punsubscribe = cmd("PUNSUBSCRIBE").get_packed_command();

            // Execute commands
            connection.con.send_bytes(&unsubscribe)?;
            connection.con.send_bytes(&punsubscribe)?;
        }

        // Receive responses
        //
        // There will be at minimum two responses - 1 for each of punsubscribe and unsubscribe
        // commands. There may be more responses if there are active subscriptions. In this case,
        // messages are received until the _subscription count_ in the responses reach zero.
        let mut received_unsub = false;
        let mut received_punsub = false;
        loop {
            let res: (Vec<u8>, (), isize) =
                from_redis_value(&connection.recv_response::<TcpStream>()?)?;

            match res.0.first() {
                Some(&b'u') => received_unsub = true,
                Some(&b'p') => received_punsub = true,
                _ => (),
            }

            if received_unsub && received_punsub && res.2 == 0 {
                break;
            }
        }

        // Finally, the connection is back in its normal state since all subscriptions were
        // cancelled *and* all unsubscribe messages were received.
        Ok(())
    }

    #[handle_request]
    /// receive messages from any of the subscribed topics or patterns
    pub fn receive(&mut self) -> RedisResult<Msg> {
        let next = loop {
            let polled = self.connection.recv_response::<TcpStream>()?;
            match Confirmation::check_confirmation(&polled) {
                Some(confirmation) => {
                    println!("Received some confirmation {:?}", confirmation);
                    continue;
                }
                None => break polled,
            };
        };
        // println!("RECEIVED NEXT {:?}", next);
        // make sure we just consume "subscription success" messages
        match Msg::from_value(&next) {
            Some(msg) => Ok(msg),
            None => Err(RedisError::from((
                ErrorKind::TypeError,
                "Failed to parse message",
            ))),
        }
    }
}
