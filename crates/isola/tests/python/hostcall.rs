use std::time::{Duration, Instant};

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

/// Concurrent host calls must overlap, not serialize. The script fires 8 host
/// calls that each sleep 200ms host-side via `asyncio.gather`. Serialized that
/// is ~1600ms; concurrently it is ~200ms. We assert the wall-clock is well
/// under the serial figure.
#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_concurrent_hostcalls_overlap() -> Result<()> {
    const COUNT: usize = 8;
    const DELAY_MS: i64 = 200;

    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "import asyncio\n\
             from sandbox.asyncio import hostcall\n\
             async def main():\n\
             \tcalls = [hostcall(\"delay\", 200) for _ in range(8)]\n\
             \treturn await asyncio.gather(*calls)",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate concurrency script")?;

    let started = Instant::now();
    let output = match tokio::time::timeout(Duration::from_secs(10), sandbox.call("main", [])).await
    {
        Ok(result) => result.context("failed to call concurrency function")?,
        Err(_) => return Err(anyhow::anyhow!("sandbox call timed out after 10s")),
    };
    let elapsed = started.elapsed();

    let value: Vec<i64> = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode concurrency response")?;
    assert_eq!(
        value,
        vec![DELAY_MS; COUNT],
        "every delayed call should echo"
    );

    let serial = Duration::from_millis((COUNT as u64) * (DELAY_MS as u64));
    assert!(
        elapsed < serial / 2,
        "expected concurrent host calls to overlap (elapsed {elapsed:?}, serial would be {serial:?})"
    );

    Ok(())
}
