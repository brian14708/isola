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

async fn guest_env(
    module: &isola::sandbox::SandboxTemplate,
    options: SandboxOptions,
) -> Result<serde_json::Value> {
    let mut sandbox = module
        .instantiate(TestHost::default(), options)
        .await
        .context("instantiate")?;
    sandbox
        .eval_script(
            "import os\n\
             def main():\n\
             \treturn [os.environ.get('ISOLA_TEST_ENV'), os.getenv('ISOLA_TEST_ENV'), \
             os.environb.get(b'ISOLA_TEST_ENV')]",
            isola::host::OutputTarget::discard(),
        )
        .await?;
    let out = sandbox.call("main", []).await?;
    Ok(out.result.expect("main result").to_json_value()?)
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_instance_env_not_baked_into_snapshot() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    // libc's `environ` and CPython's `os.environ` are populated during
    // pre-init; each instance must still see its own environment.
    let a = guest_env(
        &module,
        SandboxOptions::default().env("ISOLA_TEST_ENV", "a"),
    )
    .await?;
    let b = guest_env(
        &module,
        SandboxOptions::default().env("ISOLA_TEST_ENV", "b"),
    )
    .await?;
    let none = guest_env(&module, SandboxOptions::default()).await?;
    assert_eq!(a, serde_json::json!(["a", "a", "YQ=="]));
    assert_eq!(b, serde_json::json!(["b", "b", "Yg=="]));
    assert_eq!(none, serde_json::json!([null, null, null]));
    Ok(())
}
