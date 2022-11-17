use lunatic::{self, Mailbox};

#[lunatic::main]
fn main(_: Mailbox<()>) {
    let client = lunatic_redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_connection().unwrap();

    // test script
    let script = lunatic_redis::Script::new(
        r"
return tonumber(ARGV[1]) + tonumber(ARGV[2]);
",
    );
    let result: isize = script.arg(1).arg(2).invoke(&mut con).unwrap();
    println!("GOT SCRIPT RESULT {:?}", result);
    assert_eq!(result, 3);
}
