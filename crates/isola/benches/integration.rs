#![allow(dead_code)]

#[path = "../tests/js/common.rs"]
mod js_common;
#[path = "../tests/python/common.rs"]
mod python_common;

#[path = "integration/js.rs"]
mod js;
#[path = "integration/python.rs"]
mod python;

use criterion::{Criterion, criterion_group, criterion_main};

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
