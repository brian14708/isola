//! State baked into the wizer snapshot at template build time must not leak
//! identical values into every sandbox instantiated from it.
use anyhow::{Context, Result};
use isola::sandbox::SandboxOptions;

use super::common::{TestHost, build_module};

async fn eval_main(module: &isola::sandbox::SandboxTemplate) -> Result<String> {
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("instantiate")?;
    sandbox
        .eval_script(
            "import random\ndef main():\n\treturn random.random()",
            isola::host::OutputTarget::discard(),
        )
        .await?;
    let out = sandbox.call("main", []).await?;
    Ok(out.result.expect("main result").to_json()?)
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_random_not_replayed_across_sandboxes() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    // The prelude chain imports `random` during pre-init, baking its seeded
    // state into the snapshot; the runtime re-seeds it on the first export
    // call. Two sandboxes from one template must not replay the same sequence.
    let a = eval_main(&module).await?;
    let b = eval_main(&module).await?;
    assert_ne!(a, b);
    Ok(())
}
