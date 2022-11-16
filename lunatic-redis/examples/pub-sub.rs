extern crate lunatic_db;

use std::time::Duration;

use lunatic::{process::ProcessRef, Mailbox, Process};
use lunatic_db::redis::{self, Commands};
use redis::RedisPubSub;

#[lunatic::main]
fn main(_: Mailbox<()>) {
    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut publish_conn = client.get_connection().unwrap();
    // this process can keep reading the various subscriptions and process them
    let _sub = lunatic::spawn_link!(|| {
        let client = redis::Client::open("redis://127.0.0.1/").unwrap();
        let mut subscribe_conn = client.get_connection().unwrap().as_pubsub();

        subscribe_conn.subscribe("wavephone").unwrap();

        let pubsub_msg = subscribe_conn.receive().unwrap();
        let pubsub_msg: String = pubsub_msg.get_payload().unwrap();
        assert_eq!(&pubsub_msg, "banana");
        println!("[subscriber] GOT MESSAGE {:?}", pubsub_msg);
    });

    // allow for some time to establish the connection
    lunatic::sleep(Duration::from_secs(1));
    publish_conn
        .publish::<&str, &str, ()>("wavephone", "banana")
        .unwrap();
}

// process that handles messages from different topics
struct Chat {
    reader: Process<()>,
    pubsub: RedisPubSub,
    this: ProcessRef<Chat>,
}
