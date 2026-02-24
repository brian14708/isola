use criterion::{Criterion, black_box};
use isola::{host::NoopOutputSink, sandbox::SandboxOptions};
use tokio::runtime::Runtime;

pub fn bench(c: &mut Criterion, runtime: &Runtime) {
    let python_module = runtime
        .block_on(super::python_common::build_module())
        .expect("failed to build Python integration module");
    if let Some(python_module) = python_module {
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
                    "def main():\n\treturn 'hello world'",
                    NoopOutputSink::shared(),
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
    } else {
        eprintln!(
            "skipping integration/python benchmark: missing artifacts. Build with `cargo xtask build-all`."
        );
    }
}
