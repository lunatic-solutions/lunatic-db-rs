extern crate lunatic_db;
use lunatic::{spawn_link, Mailbox};
use lunatic_db::redis::{self, Commands};

fn set_mget() -> redis::RedisResult<(String, Vec<u8>)> {
    println!("CONNECTED");
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;

    let _: () = con.set("key1", b"foo")?;

    let _ = redis::cmd("SET")
        .arg(&["key2", "bar"])
        .query::<String>(&mut con)
        .expect("Should have SET key2");

    let result = redis::cmd("MGET")
        .arg(&["key1", "key2"])
        .query::<(String, Vec<u8>)>(&mut con);
    assert_eq!(result, Ok(("foo".to_string(), b"bar".to_vec())));
    result
}

#[lunatic::main]
fn main(_: Mailbox<()>) {
    let result = spawn_link!(@task
        || {
            set_mget()
        }
    );
    println!("GOT MGET {:?}", result.receive().1);
}
