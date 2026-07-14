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

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_unobserved_raw_hostcall_does_not_block() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "import _isola_sys\n\
             from sandbox.asyncio import hostcall\n\
             async def main():\n\
             \t_isola_sys.hostcall('delay', 0)\n\
             \treturn await hostcall('delay', 20)",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate raw hostcall script")?;

    let output = tokio::time::timeout(Duration::from_secs(2), sandbox.call("main", []))
        .await
        .context("raw hostcall without waiter blocked awaited call")?
        .context("failed to call raw hostcall function")?;
    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode raw hostcall result")?;
    assert_eq!(value, 20);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_stale_handle_cannot_release_replacement() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "import _isola_sys\n\
             def main():\n\
             \tfirst = _isola_sys.hostcall(\"echo\", 1)\n\
             \tfirst.wait()\n\
             \tsecond = _isola_sys.hostcall(\"echo\", 2)\n\
             \tfirst.release()\n\
             \treturn second.wait()",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate stale handle script")?;

    let output = sandbox
        .call("main", [])
        .await
        .context("stale handle released the replacement operation")?;
    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode stale handle result")?;
    assert_eq!(value, 2);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_high_fanout_hostcalls_complete() -> Result<()> {
    const COUNT: usize = 512;

    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = format!(
        "import asyncio\n\
         from sandbox.asyncio import hostcall\n\
         async def main():\n\
         \tresult = await asyncio.gather(\n\
         \t\t*(hostcall('echo', i) for i in range({COUNT}))\n\
         \t)\n\
         \treturn [len(result), result[0], result[-1]]"
    );
    sandbox
        .eval_script(&script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate high-fanout hostcall script")?;

    let output = tokio::time::timeout(Duration::from_secs(5), sandbox.call("main", []))
        .await
        .context("high-fanout hostcall test timed out")?
        .context("failed to call high-fanout hostcall function")?;
    let value: Vec<usize> = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode high-fanout hostcall result")?;
    assert_eq!(value, vec![COUNT, 0, COUNT - 1]);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_async_generator_preserves_in_flight_hostcall() -> Result<()> {
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
             \tslow = asyncio.create_task(hostcall('delay', 200))\n\
             \tfast = await hostcall('delay', 10)\n\
             \tyield fast\n\
             \tyield await slow",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate async generator hostcall script")?;

    let output = tokio::time::timeout(Duration::from_secs(5), sandbox.call("main", []))
        .await
        .context("async generator hostcall test timed out")?
        .context("failed to call async generator hostcall function")?;
    let values = output
        .items
        .iter()
        .map(isola::value::Value::to_serde::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode async generator hostcall outputs")?;
    assert_eq!(values, vec![10, 200]);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_expired_timer_does_not_leave_ready_sleep() -> Result<()> {
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
             \tloop = asyncio.get_running_loop()\n\
             \tloop.call_later(1e-9, lambda: None)\n\
             \treturn await hostcall('echo', 42)",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate expired timer script")?;

    let output = tokio::time::timeout(Duration::from_secs(2), sandbox.call("main", []))
        .await
        .context("expired timer test timed out")?
        .context("expired timer left a ready sleep registered")?;
    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode expired timer result")?;
    assert_eq!(value, 42);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_timeout_cancels_slow_hostcall_promptly() -> Result<()> {
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
             \tstart = asyncio.get_running_loop().time()\n\
             \ttry:\n\
             \t\tawait asyncio.wait_for(hostcall('delay', 800), timeout=0.05)\n\
             \texcept TimeoutError:\n\
             \t\treturn asyncio.get_running_loop().time() - start\n\
             \treturn -1.0",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate hostcall timeout script")?;

    let started = Instant::now();
    let output = tokio::time::timeout(Duration::from_secs(5), sandbox.call("main", []))
        .await
        .context("hostcall timeout test timed out")?
        .context("failed to call hostcall timeout function")?;
    let host_elapsed = started.elapsed();
    let guest_elapsed: f64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode timeout elapsed time")?;

    assert!(guest_elapsed >= 0.0, "slow hostcall unexpectedly completed");
    assert!(
        host_elapsed < Duration::from_millis(400),
        "timeout waited for the slow hostcall: {host_elapsed:?}"
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_cancelled_waiter_releases_hostcall() -> Result<()> {
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
             import _isola_sys\n\
             async def main():\n\
             \toperation = _isola_sys.hostcall('delay', 800)\n\
             \twaiter = asyncio.get_running_loop().subscribe(operation)\n\
             \twaiter.cancel()\n\
             \tawait asyncio.sleep(0)\n\
             \ttry:\n\
             \t\toperation.wait()\n\
             \texcept TypeError:\n\
             \t\treturn True\n\
             \treturn False",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate waiter cancellation script")?;

    let started = Instant::now();
    let output = tokio::time::timeout(Duration::from_secs(2), sandbox.call("main", []))
        .await
        .context("cancelled waiter test timed out")?
        .context("failed to call waiter cancellation function")?;
    let released: bool = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode waiter cancellation result")?;

    assert!(released, "cancelled waiter retained its hostcall handle");
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "cancelled waiter drove the released hostcall"
    );

    Ok(())
}
