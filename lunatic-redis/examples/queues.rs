use std::time::Duration;

use lunatic::{sleep, spawn_link, Mailbox};
use lunatic_db::redis::{self, Commands};

fn fetch_an_integer() -> redis::RedisResult<isize> {
    // connect to redis
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    // throw away the result, just make sure it does not fail
    let _: () = con.set("my_key", 42)?;
    // read back the key and return it.  Because the return value
    // from the function is a result for integer this will automatically
    // convert into one.
    con.get("my_key")
}

fn push_queue(value: u32) -> redis::RedisResult<isize> {
    // connect to redis
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    // throw away the result, just make sure it does not fail
    con.rpush("my_queue", value)
}

fn push_queue_timeout(timeout: Duration, value: u32) -> redis::RedisResult<isize> {
    // connect to redis
    println!("Starting timeout");
    sleep(timeout);
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    println!("Pushing {} to queue", value);
    // throw away the result, just make sure it does not fail
    con.rpush("my_queue", value)
}

fn poll_value() -> redis::RedisResult<(String, u32)> {
    // connect to redis
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    // timeout for two seconds
    con.blpop("my_queue", 2)
}

#[lunatic::main]
fn main(_: Mailbox<()>) {
    let proc = spawn_link!(@task
        || {
            fetch_an_integer()
        }
    );
    let (_, my_key) = proc.receive();
    println!("Fetched my_key from redis {:?}", my_key);

    // try to pop without any values
    println!("POLLING FIRST {:?}", poll_value());

    // push the pop
    println!("PUSHING INTO QUEUE {:?}", push_queue(55));
    println!("POLLING 55 {:?}", poll_value());

    // poll before pushing the value
    spawn_link!(|| {
        let _ = push_queue_timeout(Duration::from_secs(1), 101);
    });
    println!("POLLING LATE VALUE {:?}", poll_value());

    // push a bunch of data
    for i in 1..100 {
        push_queue(i).unwrap();
        let (_, polled) = poll_value().unwrap();
        assert_eq!(i, polled);
    }
    println!("Pushed+Polled 100 values");
}
