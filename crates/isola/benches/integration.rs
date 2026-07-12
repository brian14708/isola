#![expect(dead_code)]

use std::fmt::Write as _;

#[path = "../tests/js/common.rs"]
mod js_common;
#[path = "../tests/python/common.rs"]
mod python_common;

#[path = "integration/js.rs"]
mod js;
#[path = "integration/python.rs"]
mod python;

use criterion::{Criterion, criterion_group, criterion_main};

fn records_json() -> String {
    let mut records = String::from("[");
    for index in 0..200 {
        if index != 0 {
            records.push(',');
        }
        write!(
            records,
            r#"{{"id":{index},"owner":"team-{}","active":{},"amount":{}}}"#,
            index % 12,
            index % 3 != 0,
            (index * 17) % 10_000,
        )
        .expect("writing to a string cannot fail");
    }
    records.push(']');
    records
}

fn bench_integration(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    python::bench(c, &runtime);
    js::bench(c, &runtime);
}

criterion_group!(benches, bench_integration);
criterion_main!(benches);
