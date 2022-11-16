use lunatic::{self, Mailbox};
use lunatic_redis::{Commands, Iter};

#[lunatic::main]
async fn main(_: Mailbox<()>) {
    let client = lunatic_redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_connection().unwrap();

    con.set::<&str, &[u8; 3], String>("async-key1", b"foo")
        .unwrap();
    con.set::<&str, &[u8; 3], String>("async-key2", b"foo")
        .unwrap();

    let iter: Iter<String> = con.scan().unwrap();
    let mut keys: Vec<String> = iter.collect();

    keys.sort();

    assert_eq!(
        keys,
        vec!["async-key1".to_string(), "async-key2".to_string()]
    );
}
