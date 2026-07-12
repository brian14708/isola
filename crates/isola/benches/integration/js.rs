use std::hint::black_box;

use criterion::{BatchSize, Criterion};
use isola::{
    host::NoopOutputSink,
    sandbox::{Arg, Sandbox, SandboxOptions},
    value::Value,
};
use tokio::runtime::Runtime;

pub fn bench(c: &mut Criterion, runtime: &Runtime) {
    let js_module = runtime
        .block_on(super::js_common::build_module())
        .expect("failed to build JS integration module");
    if let Some(js_module) = js_module {
        c.bench_function("integration/js/sandbox_create", |b| {
            b.iter(|| {
                let sandbox = runtime
                    .block_on(js_module.instantiate(
                        super::js_common::TestHost::default(),
                        SandboxOptions::default(),
                    ))
                    .expect("failed to instantiate javascript sandbox");
                black_box(sandbox);
            });
        });

        let mut js_sandbox = runtime.block_on(async {
            let mut sandbox = js_module
                .instantiate(
                    super::js_common::TestHost::default(),
                    SandboxOptions::default(),
                )
                .await
                .expect("failed to instantiate javascript sandbox");
            sandbox
                .eval_script(
                    "function main() { return 'hello world'; }\n\
                     function summarize(records) {\n\
                       const active = records.filter((record) => record.active);\n\
                       return {\n\
                         count: active.length,\n\
                         total: active.reduce((sum, record) => sum + record.amount, 0),\n\
                         owners: [...new Set(active.map((record) => record.owner))].sort(),\n\
                       };\n\
                     }\n\
                     function consume(payload) { return payload.length; }\n\
                     async function roundtrip(payload) { return await hostcall('echo', payload); }",
                    NoopOutputSink::shared(),
                )
                .await
                .expect("failed to evaluate javascript script");
            sandbox
        });

        c.bench_function("integration/js", |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let output = js_sandbox
                        .call("main", [])
                        .await
                        .expect("failed to call javascript main");
                    let value: String = output
                        .result
                        .as_ref()
                        .expect("missing javascript result")
                        .to_serde()
                        .expect("failed to decode javascript result");
                    black_box(value);
                });
            });
        });

        let records = Value::from_json_str(&super::records_json())
            .expect("failed to encode benchmark records");
        c.bench_function("integration/js/records", |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let output = js_sandbox
                        .call("summarize", [Arg::Positional(records.clone())])
                        .await
                        .expect("failed to summarize javascript records");
                    black_box(output.result.expect("missing javascript summary"));
                });
            });
        });

        bench_large_values(c, runtime, &mut js_sandbox);
    } else {
        eprintln!(
            "skipping integration/js benchmark: missing artifacts. Build with `cargo xtask build-all`."
        );
    }
}

fn bench_large_values(
    c: &mut Criterion,
    runtime: &Runtime,
    sandbox: &mut Sandbox<super::js_common::TestHost>,
) {
    let large_cbor = Value::from_serde(&"x".repeat(1024 * 1024))
        .expect("failed to encode large benchmark argument")
        .into_cbor()
        .to_vec();
    for (benchmark, function) in [
        ("integration/js/large_argument", "consume"),
        ("integration/js/large_hostcall", "roundtrip"),
    ] {
        c.bench_function(benchmark, |b| {
            b.iter_batched(
                || Value::from_cbor(large_cbor.clone()),
                |payload| {
                    runtime.block_on(async {
                        let output = sandbox
                            .call(function, [Arg::Positional(payload)])
                            .await
                            .expect("failed to process large javascript argument");
                        black_box(output.result.expect("missing javascript result"));
                    });
                },
                BatchSize::LargeInput,
            );
        });
    }
}
