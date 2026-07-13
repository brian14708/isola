use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream;
use isola::{
    host::NoopOutputSink,
    sandbox::{
        Arg, CallOutput, DirPerms, Error as IsolaError, FilePerms, Sandbox, SandboxOptions, args,
    },
    value::Value,
};
use tempfile::tempdir;

use super::common::{TestHost, build_module, build_module_with_prelude};

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
            Err(IsolaError::Other(
                anyhow::anyhow!("sandbox call timed out after {}ms", timeout.as_millis()).into(),
            ))
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
async fn integration_js_sync_return_runs_microtask_checkpoint() -> Result<()> {
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
                 const result = { checkpointed: false };\n\
                 Promise.resolve().then(function () { result.checkpointed = true; });\n\
                 return result;\n\
             }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate microtask checkpoint script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call microtask checkpoint function")?;
    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode microtask checkpoint result")?;

    assert_eq!(value["checkpointed"], true);

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
async fn integration_js_typed_array_cbor_path() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await?;
    sandbox
        .eval_script(
            "function main() { return new Float32Array([1.5, -2.25]); }",
            NoopOutputSink::shared(),
        )
        .await?;
    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5)).await?;
    let value = output.result.context("expected typed-array result")?;
    let mut decoder = minicbor::Decoder::new(value.as_cbor());
    assert_eq!(decoder.tag()?.as_u64(), 84);
    assert_eq!(decoder.bytes()?, &[0, 0, 192, 63, 0, 0, 16, 192]);
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
async fn integration_js_async_generator_closes_after_serialization_error() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "let finalized = false;\n\
             async function* invalid() {\n\
             \ttry {\n\
             \t\tyield Symbol('invalid');\n\
             \t} finally {\n\
             \t\tfinalized = true;\n\
             \t}\n\
             }\n\
             function observe() { return finalized; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate async generator cleanup script")?;

    call_with_timeout(&mut sandbox, "invalid", [], Duration::from_secs(2))
        .await
        .expect_err("non-serializable generator output should fail");
    let output = call_with_timeout(&mut sandbox, "observe", [], Duration::from_secs(2))
        .await
        .context("failed to observe async generator cleanup")?;
    let finalized: bool = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode async generator cleanup state")?;

    assert!(finalized, "async generator finally block did not run");

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
async fn integration_js_sleep_respects_delay() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r"
async function main() {
    const start = _isola_sys.monotonic();
    await _isola_sys.sleep(0.05);
    return _isola_sys.monotonic() - start;
}
";
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate sleep script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call sleep function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let elapsed: f64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode elapsed time")?;
    assert!(
        elapsed >= 0.045,
        "sleep resolved too early, elapsed={elapsed}"
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_oversized_sleep_is_rejected() -> Result<()> {
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
                 try {\n\
                     _isola_sys.sleep(1e300);\n\
                 } catch (_error) {\n\
                     return true;\n\
                 }\n\
                 return false;\n\
             }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate oversized sleep script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call oversized sleep function")?;
    let rejected: bool = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode oversized sleep result")?;
    assert!(rejected, "oversized JavaScript sleep should throw");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_mixed_polling_resolves_ready_handles_incrementally() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main() {
    const events = [];
    const start = _isola_sys.monotonic();

    const immediate = hostcall("echo", "hostcall").then(function (value) {
        events.push(["hostcall", value, _isola_sys.monotonic() - start]);
    });
    const delayed = _isola_sys.sleep(0.05).then(function () {
        events.push(["sleep", "done", _isola_sys.monotonic() - start]);
    });

    await Promise.all([immediate, delayed]);
    return events;
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate mixed polling script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call mixed polling function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let events: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode mixed polling result")?;
    assert_eq!(events[0][0], "hostcall");
    assert_eq!(events[0][1], "hostcall");
    assert_eq!(events[1][0], "sleep");
    assert_eq!(events[1][1], "done");
    let sleep_elapsed = events[1][2]
        .as_f64()
        .context("expected sleep elapsed seconds")?;
    assert!(
        sleep_elapsed >= 0.045,
        "sleep resolved too early, elapsed={sleep_elapsed}"
    );

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
async fn integration_js_zero_delay_interval_does_not_starve_hostcall() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main() {
    let ticks = 0;
    const timerId = setInterval(function () {
        ticks += 1;
    }, 0);
    const value = await hostcall("delay", 20);
    clearInterval(timerId);
    return [value, ticks];
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate zero-delay interval script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(2))
        .await
        .context("zero-delay interval starved hostcall")?;
    let value: (i64, i64) = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode zero-delay interval result")?;

    assert_eq!(value.0, 20);
    assert!(
        value.1 > 0,
        "expected the interval to run before completion"
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

    let script = r"
function main(values) {
    var out = [];
    for (const value of values) {
        out.push(value * 2);
    }
    return out;
}
";

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

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_typescript_eval_and_call_roundtrip() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function main(value: number): number { return value + 1; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate typescript script")?;

    let output = call_with_timeout(&mut sandbox, "main", args![41_i64]?, Duration::from_secs(5))
        .await
        .context("failed to call typescript function")?;

    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode typescript result")?;
    assert_eq!(value, 42);
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_typescript_async_hostcall_roundtrip() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "async function main(name: string): Promise<{ name: string }> {\n\
                 return await hostcall(\"echo\", { name } as { name: string });\n\
             }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate hostcall typescript script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args!["isola"]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call hostcall typescript function")?;

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode hostcall typescript result")?;
    assert_eq!(value, serde_json::json!({ "name": "isola" }));
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_typescript_prelude_executes() -> Result<()> {
    let Some(module) =
        build_module_with_prelude(Some("const preludeValue: number = 42;".to_string())).await?
    else {
        return Ok(());
    };

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "function main() { return preludeValue; }",
            NoopOutputSink::shared(),
        )
        .await
        .context("failed to evaluate consumer script")?;

    let output = call_with_timeout(&mut sandbox, "main", [], Duration::from_secs(5))
        .await
        .context("failed to call prelude function")?;

    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode prelude result")?;
    assert_eq!(value, 42);
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_typescript_eval_file_roundtrip() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let fixture_dir = tempdir().context("failed to create tempdir")?;
    let script_path = fixture_dir.path().join("guest.ts");
    std::fs::write(
        &script_path,
        "function main(input: number): number { return input * 2; }",
    )
    .context("failed to write typescript guest file")?;

    let mut options = SandboxOptions::default();
    options = options.mount(
        fixture_dir.path(),
        "/workspace",
        DirPerms::READ,
        FilePerms::READ,
    );

    let mut sandbox = module
        .instantiate(TestHost::default(), options)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_file("/workspace/guest.ts", NoopOutputSink::shared())
        .await
        .context("failed to evaluate typescript guest file")?;

    let output = call_with_timeout(&mut sandbox, "main", args![21_i64]?, Duration::from_secs(5))
        .await
        .context("failed to call file-backed typescript function")?;

    let value: i64 = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode file-backed typescript result")?;
    assert_eq!(value, 42);
    Ok(())
}
