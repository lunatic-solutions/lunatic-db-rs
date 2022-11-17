use lunatic::{self, Mailbox};
use lunatic_redis::Commands;

#[lunatic::main]
fn main(_: Mailbox<()>) {
    let client = lunatic_redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_connection().unwrap();

    let (k1, k2): (i32, i32) = lunatic_redis::pipe()
        .cmd("SET")
        .arg("key_1")
        .arg(42)
        .ignore()
        .cmd("SET")
        .arg("key_2")
        .arg(43)
        .ignore()
        .cmd("GET")
        .arg("key_1")
        .cmd("GET")
        .arg("key_2")
        .query(&mut con)
        .unwrap();

    println!("GOT PIPED RESULTS {:?} | {:?}", k1, k2);

    let key = "the_key";
    let (new_val,): (isize,) = lunatic_redis::transaction(&mut con, &[key], |con, pipe| {
        let old_val: isize = con.get(key)?;
        pipe.set(key, old_val + 1).ignore().get(key).query(con)
    })
    .unwrap();

    println!("GOT TRANSACTION {:?}", new_val);
}
