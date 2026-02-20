use std::time::Duration;

use anyhow::{Context, Result};
use isola::TRACE_TARGET_SCRIPT;
use isola::cbor::from_cbor;
use isola::trace::collect::{CollectLayer, CollectSpanExt};
use tracing::{info_span, level_filters::LevelFilter};
use tracing_subscriber::{Registry, layer::SubscriberExt};

use super::common::{TestHost, TraceCollector, build_module, call_collect, cbor_arg};

#[tokio::test]
async fn integration_python_eval_and_call_roundtrip() -> Result<()> {
    let collector = TraceCollector::default();
    let subscriber = Registry::default().with(CollectLayer::default());
    let _guard = tracing::subscriber::set_default(subscriber);

    let root = info_span!("integration_python_eval_and_call_roundtrip");
    root.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector.clone())
        .ok_or_else(|| anyhow::anyhow!("failed to install trace collector"))?;
    let _root = root.enter();

    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\tprint('trace-print')\n\treturn 42")
        .await
        .context("failed to evaluate script")?;

    let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed to call function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: i64 = from_cbor(state.end[0].as_ref()).context("failed to decode end output")?;
    assert_eq!(value, 42);

    let events = collector.events();
    let has_print = events.iter().any(|e| {
        e.name == "log"
            && e.properties
                .iter()
                .any(|(k, v)| *k == "log.context" && v == "stdout")
            && e.properties
                .iter()
                .any(|(k, v)| *k == "log.output" && v.contains("trace-print"))
    });
    assert!(
        has_print,
        "expected trace event for print output, events: {events:?}"
    );

    Ok(())
}

#[tokio::test]
async fn integration_python_streaming_output() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\tfor i in range(3):\n\t\tyield i")
        .await
        .context("failed to evaluate streaming script")?;

    let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed to call streaming function")?;

    assert_eq!(state.partial.len(), 3, "expected three partial outputs");
    let mut values = Vec::with_capacity(state.partial.len());
    for item in &state.partial {
        values.push(from_cbor::<i64>(item.as_ref()).context("failed to decode partial output")?);
    }
    assert_eq!(values, vec![0, 1, 2]);

    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    if !state.end[0].is_empty() {
        let end_value: Option<i64> =
            from_cbor(state.end[0].as_ref()).context("failed to decode end output")?;
        assert_eq!(end_value, None, "expected empty end output or null value");
    }

    Ok(())
}

#[tokio::test]
async fn integration_python_argument_cbor_path() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main(i, s):\n\treturn (i + 1, s.upper())")
        .await
        .context("failed to evaluate argument script")?;

    let args = vec![cbor_arg(None, "41")?, cbor_arg(Some("s"), "\"hello\"")?];
    let state = call_collect(&mut sandbox, "main", args, Duration::from_secs(2))
        .await
        .context("failed to call argument function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: (i64, String) =
        from_cbor(state.end[0].as_ref()).context("failed to decode argument result")?;
    assert_eq!(value, (42, "HELLO".to_string()));
    Ok(())
}

#[tokio::test]
async fn integration_python_reinstantiate_smoke() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    for expected in [7_i64, 11_i64] {
        let mut sandbox = module
            .instantiate(None, TestHost::default())
            .await
            .context("failed to instantiate sandbox")?;

        sandbox
            .eval_script(format!("def main():\n\treturn {expected}"))
            .await
            .context("failed to evaluate script")?;

        let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
            .await
            .context("failed to call function")?;
        assert_eq!(state.end.len(), 1, "expected exactly one end output");
        let value: i64 =
            from_cbor(state.end[0].as_ref()).context("failed to decode roundtrip output")?;
        assert_eq!(value, expected);
    }

    Ok(())
}

#[tokio::test]
async fn integration_python_guest_exception_surface() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\traise RuntimeError(\"boom\")")
        .await
        .context("failed to evaluate exception script")?;

    let err = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .expect_err("expected exception from guest function");
    let isola::Error::Guest { message, .. } = err else {
        panic!("expected guest error, got {err:?}");
    };
    assert!(
        message.contains("boom"),
        "unexpected error message: {message}"
    );

    Ok(())
}

#[tokio::test]
async fn integration_python_state_persists_within_sandbox() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "counter = 0\n\
             def main():\n\
             \tglobal counter\n\
             \tcounter += 1\n\
             \treturn counter",
        )
        .await
        .context("failed to evaluate stateful script")?;

    let first = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed first stateful call")?;
    let second = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed second stateful call")?;

    assert_eq!(first.end.len(), 1, "expected exactly one first end output");
    assert_eq!(
        second.end.len(),
        1,
        "expected exactly one second end output"
    );
    let first_v: i64 = from_cbor(first.end[0].as_ref()).context("failed to decode first value")?;
    let second_v: i64 =
        from_cbor(second.end[0].as_ref()).context("failed to decode second value")?;
    assert_eq!(first_v, 1);
    assert_eq!(second_v, 2);

    Ok(())
}

#[tokio::test]
async fn integration_python_call_timeout() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(None, TestHost::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\twhile True:\n\t\tpass")
        .await
        .context("failed to evaluate timeout script")?;

    let err = call_collect(&mut sandbox, "main", vec![], Duration::from_millis(1))
        .await
        .expect_err("expected timeout while executing guest function");
    let isola::Error::Wasm(cause) = err else {
        panic!("expected wasm timeout error, got {err:?}");
    };
    let message = cause.to_string().to_ascii_lowercase();
    assert!(
        message.contains("interrupt") || message.contains("deadline") || message.contains("epoch"),
        "unexpected timeout error message: {cause}"
    );

    Ok(())
}
