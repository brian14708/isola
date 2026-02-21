use std::time::Duration;

use anyhow::{Context, Result};
use isola::{host::NoopOutputSink, sandbox::SandboxOptions};

use super::common::{TestHost, build_module};

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_async_hostcall_echo() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "from sandbox.asyncio import hostcall\n\
             async def main():\n\
             \treturn await hostcall(\"echo\", [1, 2, 3])",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate async hostcall script")?;

    let output = match tokio::time::timeout(Duration::from_secs(2), sandbox.call("main", [])).await
    {
        Ok(result) => result.context("failed to call async hostcall function")?,
        Err(_) => {
            return Err(anyhow::anyhow!("sandbox call timed out after {}ms", 2_000));
        }
    };
    assert!(output.items.is_empty(), "expected no partial outputs");

    let value: Vec<i64> = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode hostcall response")?;
    assert_eq!(value, vec![1, 2, 3]);

    Ok(())
}
