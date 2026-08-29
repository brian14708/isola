//! State baked into the wizer snapshot at template build time must not leak
//! identical values into every sandbox instantiated from it.
use anyhow::{Context, Result};
use isola::{host::OutputTarget, sandbox::SandboxOptions};

use super::common::{TestHost, build_module};

async fn eval_main(module: &isola::sandbox::SandboxTemplate) -> Result<String> {
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("instantiate")?;
    sandbox
        .eval_script(
            "globalThis.main = () => Math.random();",
            OutputTarget::discard(),
        )
        .await?;
    let out = sandbox.call("main", []).await?;
    Ok(out.result.expect("main result").to_json()?)
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_math_random_not_replayed_across_sandboxes() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    // QuickJS seeds Math.random from the clock at context creation, so the
    // seed lands in the wizer snapshot; the runtime replaces Math.random with
    // a WASI-entropy-backed binding on the first export call.
    let a = eval_main(&module).await?;
    let b = eval_main(&module).await?;
    assert_ne!(a, b);
    Ok(())
}
