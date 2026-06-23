use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use isola::{host::NoopOutputSink, sandbox::SandboxOptions};

use super::common::{TestHost, build_module};

async fn assert_echo_hostcall(script: &str) -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate hostcall script")?;

    let output = match tokio::time::timeout(Duration::from_secs(5), sandbox.call("main", [])).await
    {
        Ok(result) => result.context("failed to call hostcall function")?,
        Err(_) => {
            return Err(anyhow::anyhow!("sandbox call timed out after 5s"));
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

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_async_hostcall_echo() -> Result<()> {
    assert_echo_hostcall(
        "async function main() {\n\
             return await hostcall('echo', [1, 2, 3]);\n\
         }",
    )
    .await
}

/// Concurrent host calls must overlap, not serialize. The script fires 8 host
/// calls that each sleep 200ms host-side via `Promise.all`. Serialized that is
/// ~1600ms; concurrently it is ~200ms. We assert the wall-clock is well under
/// the serial figure.
#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_concurrent_hostcalls_overlap() -> Result<()> {
    const COUNT: usize = 8;
    const DELAY_MS: i64 = 200;

    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = format!(
        "async function main() {{\n\
             const calls = [];\n\
             for (let i = 0; i < {COUNT}; i++) {{\n\
                 calls.push(hostcall('delay', {DELAY_MS}));\n\
             }}\n\
             return await Promise.all(calls);\n\
         }}"
    );

    sandbox
        .eval_script(&script, NoopOutputSink::shared())
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
