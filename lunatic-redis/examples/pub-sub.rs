extern crate lunatic_db;
use lunatic::{spawn_link, Mailbox};
use lunatic_db::redis::{self, Commands};

#[lunatic::main]
fn main(_: Mailbox<()>) {
    // let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    // let mut publish_conn = client.get_connection().unwrap();
    // let mut pubsub_conn = client.get_connection().unwrap().into_pubsub();

    // pubsub_conn.subscribe("wavephone");
    // let mut pubsub_stream = pubsub_conn.on_message();

    // publish_conn.publish("wavephone", "banana");

    // let pubsub_msg: String = pubsub_stream.next().unwrap().get_payload();
    // assert_eq!(&pubsub_msg, "banana");
}
