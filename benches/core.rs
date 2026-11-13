use std::{hint::black_box, time::Instant};

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use remik::{
    db::Db,
    protocol::{self, Frame, Limits},
};

fn command(args: &[&'static [u8]]) -> Vec<Bytes> {
    args.iter().copied().map(Bytes::from_static).collect()
}

fn core(c: &mut Criterion) {
    let set = command(&[b"SET", b"benchmark-key", b"benchmark-value"]);
    let mut wire = Vec::new();
    Frame::Array(set.iter().cloned().map(Frame::Bulk).collect()).encode(&mut wire);
    c.bench_function("resp2/decode_set", |b| {
        b.iter(|| {
            protocol::decode(black_box(&wire), Limits::default())
                .unwrap()
                .unwrap()
        })
    });

    let now = Instant::now();
    let mut db = Db::new(64 * 1024 * 1024);
    assert_eq!(db.execute(&set, now), Frame::Simple("OK".into()));
    let get = command(&[b"GET", b"benchmark-key"]);
    c.bench_function("engine/get_hit", |b| {
        b.iter(|| db.execute(black_box(&get), now))
    });
    c.bench_function("engine/set_replace", |b| {
        b.iter(|| db.execute(black_box(&set), now))
    });

    let response = Frame::Bulk(Bytes::from(vec![b'x'; 1024]));
    let mut output = Vec::with_capacity(response.encoded_len());
    c.bench_function("resp2/encode_1k", |b| {
        b.iter(|| {
            output.clear();
            response.encode(&mut output);
            black_box(&output);
        })
    });
}

criterion_group!(benches, core);
criterion_main!(benches);
