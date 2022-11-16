use lunatic_redis::Value;
use partial_io::{
    quickcheck_types::{GenWouldBlock, PartialWithErrors},
    PartialRead,
};
use quickcheck::{quickcheck, Gen};
mod support;
use crate::support::encode_value;

#[derive(Clone, Debug)]
struct ArbitraryValue(Value);

impl ::quickcheck::Arbitrary for ArbitraryValue {
    fn arbitrary(g: &mut Gen) -> Self {
        let size = g.size();
        ArbitraryValue(arbitrary_value(g, size))
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        match self.0 {
            Value::Nil | Value::Okay => Box::new(None.into_iter()),
            Value::Int(i) => Box::new(i.shrink().map(Value::Int).map(ArbitraryValue)),
            Value::Data(ref xs) => Box::new(xs.shrink().map(Value::Data).map(ArbitraryValue)),
            Value::Bulk(ref xs) => {
                let ys = xs
                    .iter()
                    .map(|x| ArbitraryValue(x.clone()))
                    .collect::<Vec<_>>();
                Box::new(
                    ys.shrink()
                        .map(|xs| xs.into_iter().map(|x| x.0).collect())
                        .map(Value::Bulk)
                        .map(ArbitraryValue),
                )
            }
            Value::Status(ref status) => {
                Box::new(status.shrink().map(Value::Status).map(ArbitraryValue))
            }
        }
    }
}

fn arbitrary_value(g: &mut Gen, recursive_size: usize) -> Value {
    use quickcheck::Arbitrary;
    if recursive_size == 0 {
        Value::Nil
    } else {
        match u8::arbitrary(g) % 6 {
            0 => Value::Nil,
            1 => Value::Int(Arbitrary::arbitrary(g)),
            2 => Value::Data(Arbitrary::arbitrary(g)),
            3 => {
                let size = {
                    let s = g.size();
                    usize::arbitrary(g) % s
                };
                Value::Bulk(
                    (0..size)
                        .map(|_| arbitrary_value(g, recursive_size / size))
                        .collect(),
                )
            }
            4 => {
                let size = {
                    let s = g.size();
                    usize::arbitrary(g) % s
                };

                let mut status = String::with_capacity(size);
                for _ in 0..size {
                    let c = char::arbitrary(g);
                    if c.is_ascii_alphabetic() {
                        status.push(c);
                    }
                }

                if status == "OK" {
                    Value::Okay
                } else {
                    Value::Status(status)
                }
            }
            5 => Value::Okay,
            _ => unreachable!(),
        }
    }
}

quickcheck! {
    fn partial_io_parse(input: ArbitraryValue, seq: PartialWithErrors<GenWouldBlock>) -> () {

        let mut encoded_input = Vec::new();
        encode_value(&input.0, &mut encoded_input).unwrap();

        let mut _reader = &encoded_input[..];
        let mut _partial_reader = PartialRead::new(_reader, Box::new(seq.into_iter()));
        // let mut decoder = combine::stream::Decoder::new();

        let result = lunatic_redis::parse_redis_value(_partial_reader);
        assert!(result.as_ref().is_ok(), "{}", result.unwrap_err());
        assert_eq!(
            result.unwrap(),
            input.0,
        );
    }
}
