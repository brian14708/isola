use std::hint::black_box;

use criterion::{BatchSize, Criterion};
use isola::{
    host::OutputTarget,
    sandbox::{Arg, Sandbox, SandboxOptions},
    value::Value,
};
use tokio::runtime::Runtime;

pub fn bench(c: &mut Criterion, runtime: &Runtime) {
    let python_module = runtime
        .block_on(super::python_common::build_module())
        .expect("failed to build Python integration module");
    if let Some(python_module) = python_module {
        c.bench_function("integration/python/sandbox_create", |b| {
            b.iter(|| {
                let sandbox = runtime
                    .block_on(python_module.instantiate(
                        super::python_common::TestHost::default(),
                        SandboxOptions::default(),
                    ))
                    .expect("failed to instantiate python sandbox");
                black_box(sandbox);
            });
        });

        let mut python_sandbox = runtime.block_on(async {
            let mut sandbox = python_module
                .instantiate(
                    super::python_common::TestHost::default(),
                    SandboxOptions::default(),
                )
                .await
                .expect("failed to instantiate python sandbox");
            sandbox
                .eval_script(
                    "def main():\n\treturn 'hello world'\n\n\
                     def summarize(records):\n\
                     \tactive = [r for r in records if r['active']]\n\
                     \treturn {\n\
                     \t\t'count': len(active),\n\
                     \t\t'total': sum(r['amount'] for r in active),\n\
                     \t\t'owners': sorted({r['owner'] for r in active}),\n\
                     \t}\n\n\
                     def consume(payload):\n\
                     \treturn len(payload)\n\n\
                     async def roundtrip(payload):\n\
                     \tfrom sandbox.asyncio import hostcall\n\
                     \treturn await hostcall('echo', payload)",
                    OutputTarget::discard(),
                )
                .await
                .expect("failed to evaluate python script");
            sandbox
        });

        c.bench_function("integration/python", |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let output = python_sandbox
                        .call("main", [])
                        .await
                        .expect("failed to call python main");
                    let value: String = output
                        .result
                        .as_ref()
                        .expect("missing python result")
                        .to_serde()
                        .expect("failed to decode python result");
                    black_box(value);
                });
            });
        });

        let records = Value::from_json_str(&super::records_json())
            .expect("failed to encode benchmark records");
        c.bench_function("integration/python/records", |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let output = python_sandbox
                        .call("summarize", [Arg::Positional(records.clone())])
                        .await
                        .expect("failed to summarize python records");
                    black_box(output.result.expect("missing python summary"));
                });
            });
        });

        bench_large_values(c, runtime, &mut python_sandbox);
    } else {
        eprintln!(
            "skipping integration/python benchmark: missing artifacts. Build with `cargo xtask build-all`."
        );
    }
}

fn bench_large_values(
    c: &mut Criterion,
    runtime: &Runtime,
    sandbox: &mut Sandbox<super::python_common::TestHost>,
) {
    let large_cbor = Value::from_serde(&"x".repeat(1024 * 1024))
        .expect("failed to encode large benchmark argument")
        .into_cbor()
        .to_vec();
    for (benchmark, function) in [
        ("integration/python/large_argument", "consume"),
        ("integration/python/large_hostcall", "roundtrip"),
    ] {
        c.bench_function(benchmark, |b| {
            b.iter_batched(
                || Value::from_cbor(large_cbor.clone()),
                |payload| {
                    runtime.block_on(async {
                        let output = sandbox
                            .call(function, [Arg::Positional(payload)])
                            .await
                            .expect("failed to process large python argument");
                        black_box(output.result.expect("missing python result"));
                    });
                },
                BatchSize::LargeInput,
            );
        });
    }
}
