use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream;
use isola::{
    host::NoopOutputSink,
    sandbox::{Arg, CallOutput, Error as IsolaError, Sandbox, SandboxOptions, args},
    value::Value,
};

use super::common::{TestHost, build_module};

async fn call_with_timeout<I>(
    sandbox: &mut Sandbox<TestHost>,
    function: &str,
    args: I,
    timeout: Duration,
) -> std::result::Result<CallOutput, IsolaError>
where
    I: IntoIterator<Item = Arg>,
{
    tokio::time::timeout(timeout, sandbox.call(function, args))
        .await
        .unwrap_or_else(|_| {
            Err(IsolaError::Runtime(anyhow::anyhow!(
                "sandbox call timed out after {}ms",
                timeout.as_millis()
            )))
        })
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_eval_and_call_roundtrip() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("function main() { return 42; }", NoopOutputSink::shared())
        .await
        .context("failed to evaluate script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: i64 = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode end output")?;
    assert_eq!(value, 42);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_streaming_output() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function* main() { for (let i = 0; i < 3; i++) { yield i; } }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate streaming script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call streaming function")?;

    assert_eq!(output.items.len(), 3, "expected three partial outputs");
    let mut values = Vec::with_capacity(output.items.len());
    for item in &output.items {
        values.push(
            item.to_serde::<i64>()
                .context("failed to decode partial output")?,
        );
    }
    assert_eq!(values, vec![0, 1, 2]);
    assert!(output.result.is_none(), "expected null end output");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_reinstantiate_smoke() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    for expected in [7_i64, 11_i64] {
        let mut sandbox = module
            .instantiate(TestHost::default(), SandboxOptions::default())
            .await
            .context("failed to instantiate sandbox")?;

        sandbox
            .eval_script(
                &format!("function main() {{ return {expected}; }}"),
                NoopOutputSink::shared(),
            )
            .await
            .context("failed to evaluate script")?;

        let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
            .await
            .context("failed to call function")?;
        let value: i64 = output
            .result
            .as_ref()
            .context("expected exactly one end output")?
            .to_serde()
            .context("failed to decode roundtrip output")?;
        assert_eq!(value, expected);
    }

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_guest_exception_surface() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function main() { throw new Error('boom'); }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate exception script")?;

    let err = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .expect_err("expected exception from guest function");
    let IsolaError::UserCode { message } = err else {
        panic!("expected guest error, got {err:?}");
    };
    assert!(
        message.contains("boom"),
        "unexpected error message: {message}",
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_state_persists_within_sandbox() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "let counter = 0;\nfunction main() { counter += 1; return counter; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate stateful script")?;

    let first = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed first stateful call")?;
    let second = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed second stateful call")?;

    let first_v: i64 = first
        .result
        .as_ref()
        .context("expected exactly one first end output")?
        .to_serde()
        .context("failed to decode first value")?;
    let second_v: i64 = second
        .result
        .as_ref()
        .context("expected exactly one second end output")?
        .to_serde()
        .context("failed to decode second value")?;
    assert_eq!(first_v, 1);
    assert_eq!(second_v, 2);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_argument_cbor_path() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function main(i, opts) { return [i + 1, opts.s.toUpperCase()]; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate argument script")?;

    let args = args![41_i64, s = "hello"]?;
    let output = call_with_timeout(&mut sandbox, "main", args, Duration::from_secs(5))
        .await
        .context("failed to call argument function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: (i64, String) = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode argument result")?;
    assert_eq!(value, (42, "HELLO".to_string()));
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_return_object() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function main() { return { name: 'isola', version: 1 }; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate object return script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call function")?;

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode object result")?;
    assert_eq!(value["name"], "isola");
    assert_eq!(value["version"], 1);
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_emit_partial_results() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function main() {\n\
                 _isola_sys.emit('partial-1');\n\
                 _isola_sys.emit('partial-2');\n\
                 return 'final';\n\
             }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate emit script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call emit function")?;

    assert_eq!(output.items.len(), 2, "expected two partial outputs");
    let p1: String = output.items[0]
        .to_serde()
        .context("failed to decode partial 1")?;
    let p2: String = output.items[1]
        .to_serde()
        .context("failed to decode partial 2")?;
    assert_eq!(p1, "partial-1");
    assert_eq!(p2, "partial-2");

    let result: String = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode final result")?;
    assert_eq!(result, "final");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_async_function_basic() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "async function main() { return 42; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate async script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call async function")?;

    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode async result")?;
    assert_eq!(value, 42);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_async_generator_streaming() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "async function* main() {\n\
                 yield 'a';\n\
                 yield 'b';\n\
                 yield 'c';\n\
             }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate async generator script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call async generator function")?;

    assert_eq!(output.items.len(), 3, "expected three partial outputs");
    let values: Vec<String> = output
        .items
        .iter()
        .map(|item| item.to_serde().unwrap())
        .collect();
    assert_eq!(values, vec!["a", "b", "c"]);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_promise_all_pure() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    // Promise.all with pure async functions (no I/O) to test basic event loop
    sandbox
        .eval_script(
            "async function main() {\n\
                 return await Promise.resolve(99);\n\
             }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate promise.all script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call promise.all function")?;

    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode promise.all result")?;
    assert_eq!(value, 99);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_set_timeout_and_clear_timeout() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main() {
    return await new Promise(function (resolve) {
        var events = [];

        var canceledId = setTimeout(function () {
            events.push("canceled");
        }, 10);
        clearTimeout(canceledId);

        setTimeout(function (label, value) {
            events.push(label + ":" + value);
            resolve({
                hasSetTimeout: typeof setTimeout === "function",
                hasClearTimeout: typeof clearTimeout === "function",
                events: events,
            });
        }, 0, "ran", 7);
    });
}
"#;

    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate timeout script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call timeout function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode timeout result")?;

    assert_eq!(value["hasSetTimeout"], true);
    assert_eq!(value["hasClearTimeout"], true);
    assert_eq!(value["events"], serde_json::json!(["ran:7"]));

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_set_interval_and_clear_interval() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main() {
    return await new Promise(function (resolve) {
        var events = [];
        var timerId = setInterval(function (label) {
            events.push(label + ":" + events.length);
            if (events.length === 3) {
                clearInterval(timerId);
                resolve({
                    hasSetInterval: typeof setInterval === "function",
                    hasClearInterval: typeof clearInterval === "function",
                    events: events,
                });
            }
        }, 0, "tick");
    });
}
"#;

    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate interval script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call interval function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode interval result")?;

    assert_eq!(value["hasSetInterval"], true);
    assert_eq!(value["hasClearInterval"], true);
    assert_eq!(
        value["events"],
        serde_json::json!(["tick:0", "tick:1", "tick:2"]),
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_stream_input_arg_is_iterable() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
function main(values) {
    var out = [];
    for (const value of values) {
        out.push(value * 2);
    }
    return out;
}
"#;

    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate stream-arg script")?;

    let stream_values = (1_i64..=3)
        .map(|v| Value::from_serde(&v))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to build stream values")?;
    let stream_arg = stream::iter(stream_values);
    let args = args![@stream(stream_arg)].context("failed to build stream args")?;

    let output = call_with_timeout(&mut sandbox, "main", args, Duration::from_secs(5))
        .await
        .context("failed to call stream-arg function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: Vec<i64> = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode stream-arg result")?;
    assert_eq!(value, vec![2, 4, 6]);

    Ok(())
}
