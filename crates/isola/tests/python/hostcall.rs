use std::time::Duration;

use anyhow::{Context, Result};
use isola::cbor::from_cbor;

use super::common::{TestHost, build_module, call_collect};

#[tokio::test]
async fn integration_python_async_hostcall_echo() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "from sandbox.asyncio import hostcall\n\
             async def main():\n\
             \treturn await hostcall(\"echo\", [1, 2, 3])",
        )
        .await
        .context("failed to evaluate async hostcall script")?;

    let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed to call async hostcall function")?;
    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");

    let value: Vec<i64> =
        from_cbor(state.end[0].as_ref()).context("failed to decode hostcall response")?;
    assert_eq!(value, vec![1, 2, 3]);

    Ok(())
}
