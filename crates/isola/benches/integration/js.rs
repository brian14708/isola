use criterion::{Criterion, black_box};
use isola::{host::NoopOutputSink, sandbox::SandboxOptions};
use tokio::runtime::Runtime;

pub fn bench(c: &mut Criterion, runtime: &Runtime) {
    let js_module = runtime
        .block_on(super::js_common::build_module())
        .expect("failed to build JS integration module");
    if let Some(js_module) = js_module {
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
                    "function main() { return 'hello world'; }",
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
    } else {
        eprintln!(
            "skipping integration/js benchmark: missing artifacts. Build with `cargo xtask build-all`."
        );
    }
}
