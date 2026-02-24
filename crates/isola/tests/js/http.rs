use std::time::Duration;

use anyhow::{Context, Result};
use isola::{
    host::NoopOutputSink,
    sandbox::{Arg, CallOutput, Sandbox, SandboxOptions, args},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, header, method, path},
};

use super::common::{TestHost, build_module};

async fn call_with_timeout<I>(
    sandbox: &mut Sandbox<TestHost>,
    function: &str,
    args: I,
    timeout: Duration,
) -> Result<CallOutput>
where
    I: IntoIterator<Item = Arg>,
{
    tokio::time::timeout(timeout, sandbox.call(function, args))
        .await
        .map_or_else(
            |_| {
                Err(anyhow::anyhow!(
                    "sandbox call timed out after {}ms",
                    timeout.as_millis()
                ))
            },
            |result| result.map_err(Into::into),
        )
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_client_roundtrip() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/echo"))
        .and(header("content-type", "application/json"))
        .and(body_string(r#"{"hello":"world"}"#))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"ok":true}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    let resp = await fetch(url + "/echo", {
        method: "POST",
        headers: {"content-type": "application/json"},
        body: '{"hello":"world"}'
    });
    return resp.text();
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate http fetch script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call http fetch function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");

    let value: String = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode response body")?;
    assert_eq!(value, r#"{"ok":true}"#);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_status_errors_surface() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/status/503"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/status/500"))
        .and(header("content-type", "application/json"))
        .and(body_string(r#"{"value":"test"}"#))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    let first = await fetch(url + "/status/503");
    let second = await fetch(url + "/status/500", {
        method: "POST",
        body: {value: "test"}
    });
    return [first.status, second.status];
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate status script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call status function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: (i64, i64) = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode status tuple")?;
    assert_eq!(value, (503, 500));

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_concurrent_requests() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"name":"a"}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"name":"b"}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    // Use Promise.all to verify concurrent requests work
    let script = r#"
async function main(url) {
    let [a, b] = await Promise.all([
        fetch(url + "/a"),
        fetch(url + "/b")
    ]);
    return Promise.all([a.json(), b.json()]);
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate concurrent fetch script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call concurrent fetch function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: Vec<serde_json::Value> = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode concurrent result")?;
    assert_eq!(value.len(), 2);
    assert_eq!(value[0]["name"], "a");
    assert_eq!(value[1]["name"], "b");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_json_body() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/json"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"received":true}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    let resp = await fetch(url + "/json", {
        method: "POST",
        body: {key: "value"}
    });
    return resp.json();
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate json body script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call json body function")?;

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode json response")?;
    assert_eq!(value["received"], true);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_delayed_concurrent() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    // Slow endpoint: 500ms delay
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("slow")
                .set_body_string("slow-response")
                .insert_header("content-type", "text/plain"),
        )
        .expect(1)
        .mount(&server)
        .await;
    // Fast endpoint: immediate
    Mock::given(method("GET"))
        .and(path("/fast"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("fast-response")
                .insert_header("content-type", "text/plain"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    // Both requests should complete via Promise.all with the poll-based event loop
    let script = r#"
async function main(url) {
    let [slow, fast] = await Promise.all([
        fetch(url + "/slow").then(r => r.text()),
        fetch(url + "/fast").then(r => r.text())
    ]);
    return {slow, fast};
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate delayed concurrent script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(10),
    )
    .await
    .context("failed to call delayed concurrent function")?;

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode delayed concurrent result")?;
    assert_eq!(value["slow"], "slow-response");
    assert_eq!(value["fast"], "fast-response");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_headers_and_request_input() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/headers"))
        .and(header("content-type", "application/json"))
        .and(body_string(r#"{"hello":"world"}"#))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"ok":true}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    const headers = new Headers([["X-Dup", "a"]]);
    headers.append("x-dup", "b");
    headers.set("content-type", "application/json");
    const req = new Request(url + "/headers", {
        method: "POST",
        headers,
        body: {hello: "world"},
    });

    const resp = await fetch(req);
    return {
        status: resp.status,
        ok: resp.ok,
        header: req.headers.get("x-dup"),
        body: await resp.json(),
    };
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate headers/request script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call headers/request function")?;

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode headers/request result")?;
    assert_eq!(value["status"], 200);
    assert_eq!(value["ok"], true);
    assert_eq!(value["header"], "a, b");
    assert_eq!(value["body"]["ok"], true);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_body_used_enforced() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/read-once"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"ok":true}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    const resp = await fetch(url + "/read-once");
    const first = await resp.text();
    let secondError = "";
    try {
        await resp.json();
    } catch (e) {
        secondError = String(e.message || e);
    }
    return {first, secondError, bodyUsed: resp.bodyUsed};
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate bodyUsed script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call bodyUsed function")?;

    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode bodyUsed result")?;
    assert_eq!(value["first"], r#"{"ok":true}"#);
    assert_eq!(value["bodyUsed"], true);
    let second_error = value["secondError"]
        .as_str()
        .context("expected secondError as string")?;
    assert!(
        second_error.contains("Body has already been"),
        "unexpected bodyUsed second-read error: {second_error}",
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_abort_pre_aborted_rejects() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    const controller = new AbortController();
    controller.abort("stop");
    try {
        await fetch(url + "/never", {signal: controller.signal});
        return "expected-abort";
    } catch (e) {
        return String(e.name || e);
    }
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate abort script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args!["http://example.com"]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call abort function")?;

    let value: String = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode abort result")?;
    assert_eq!(value, "AbortError");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_get_with_body_rejected() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
function main(url) {
    try {
        new Request(url + "/invalid", {method: "GET", body: "x"});
        return "expected-get-body-error";
    } catch (e) {
        return String(e.message || e);
    }
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate GET body script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args!["http://example.com"]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call GET body function")?;

    let value: String = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode GET body result")?;
    assert!(
        value.contains("GET/HEAD"),
        "unexpected GET body error message: {value}",
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_url_search_params_body() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/form"))
        .and(header(
            "content-type",
            "application/x-www-form-urlencoded;charset=UTF-8",
        ))
        .and(body_string("a=1&b=two"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    const params = new URLSearchParams({a: "1", b: "two"});
    const resp = await fetch(url + "/form", {
        method: "POST",
        body: params,
    });
    return [resp.status, await resp.text()];
}
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate URLSearchParams script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call URLSearchParams function")?;

    let value: (i64, String) = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode URLSearchParams result")?;
    assert_eq!(value.0, 200);
    assert_eq!(value.1, "ok");

    Ok(())
}
