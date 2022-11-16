extern crate lunatic_db;

use std::time::Duration;

use lunatic::{sleep, Mailbox};
use lunatic_db::redis::{self, Commands};

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

    // create hello channel first
    publish_conn
        .publish::<&str, &str, ()>("hello", "init")
        .unwrap();

    publish_conn
        .publish::<&str, &str, ()>("hallo", "init")
        .unwrap();

    publish_conn
        .publish::<&str, &str, ()>("world", "init")
        .unwrap();

    publish_conn
        .publish::<&str, &str, ()>("wörld", "init")
        .unwrap();

    // do some pattern subscription
    let _sub = lunatic::spawn_link!(|| {
        let client = redis::Client::open("redis://127.0.0.1/").unwrap();
        let mut subscribe_conn = client.get_connection().unwrap().as_pubsub();
        subscribe_conn.psubscribe("hello").unwrap();
        subscribe_conn.psubscribe("w*rld").unwrap();
        println!("SUBBED TO TOPICS");

        loop {
            let pubsub_msg = subscribe_conn.receive();
            println!("[subscriber] GOT PMESSAGE {:?}", pubsub_msg);
        }
    });

    sleep(Duration::from_secs(1));
    // publish to pattern
    publish_conn
        .publish::<&str, &str, ()>("hello", "first")
        .unwrap();
    publish_conn
        .publish::<&str, &str, ()>("world", "second")
        .unwrap();
    publish_conn
        .publish::<&str, &str, ()>("hello", "third")
        .unwrap();
    publish_conn
        .publish::<&str, &str, ()>("wörld", "fourth")
        .unwrap();

    sleep(Duration::from_secs(2));
}
