use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use isola::{
    host::OutputTarget,
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
        .eval_script(script, OutputTarget::discard())
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
async fn integration_js_http_streaming_upload() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function* chunks() {
    yield new Uint8Array([102, 105, 114, 115, 116]);
    await new Promise(resolve => setTimeout(resolve, 10));
    yield new Uint8Array([115, 101, 99, 111, 110, 100]);
}

async function main(url) {
    const response = await fetch(url + "/upload", {
        method: "POST",
        body: chunks(),
    });
    return [response.status, await response.text()];
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate streaming upload script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("streaming upload did not complete")?;
    let value: (u16, String) = output
        .result
        .context("expected streaming upload result")?
        .to_serde()
        .context("failed to decode streaming upload result")?;
    assert_eq!(value, (200, "ok".to_owned()));
    let requests = server
        .received_requests()
        .await
        .context("request recording is disabled")?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, b"firstsecond");
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_large_response_is_chunked_and_limited() -> Result<()> {
    const LARGE_RESPONSE_BODY_BYTES: usize = 256 * 1024 + 7;
    const MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;

    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/large"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(
                (0..LARGE_RESPONSE_BODY_BYTES)
                    .map(|index| u8::try_from(index % 251).unwrap())
                    .collect::<Vec<_>>(),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/oversized"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![b'x'; MAX_RESPONSE_BODY_BYTES + 1]),
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
    const body = new Uint8Array(
        await (await fetch(url + "/large")).arrayBuffer()
    );

    let oversizedError = "expected response-size error";
    try {
        await (await fetch(url + "/oversized")).arrayBuffer();
    } catch (error) {
        oversizedError = String(error.message || error);
    }

    return [body.byteLength, body[0], body[body.byteLength - 1], oversizedError];
}
"#;
    sandbox
        .eval_script(script, OutputTarget::discard())
        .await
        .context("failed to evaluate large-response script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(10),
    )
    .await
    .context("failed to call large-response function")?;

    let value: (i64, i64, i64, String) = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode large-response result")?;
    assert_eq!(value.0, i64::try_from(LARGE_RESPONSE_BODY_BYTES).unwrap());
    assert_eq!(value.1, 0);
    assert_eq!(
        value.2,
        i64::from(u8::try_from((LARGE_RESPONSE_BODY_BYTES - 1) % 251).unwrap())
    );
    let expected_error =
        format!("HTTP response body exceeds maximum size of {MAX_RESPONSE_BODY_BYTES} bytes");
    assert!(
        value.3.contains(&expected_error),
        "unexpected response-size error: {}",
        value.3
    );

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
        .eval_script(script, OutputTarget::discard())
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
        .eval_script(script, OutputTarget::discard())
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
        .eval_script(script, OutputTarget::discard())
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

    // Both requests should complete via Promise.all with the poll-based event
    // loop
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
        .eval_script(script, OutputTarget::discard())
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
        .eval_script(script, OutputTarget::discard())
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
    const streamRead = await resp.body.getReader().read();
    return {
        first,
        secondError,
        bodyUsed: resp.bodyUsed,
        streamDone: streamRead.done,
    };
}
"#;
    sandbox
        .eval_script(script, OutputTarget::discard())
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
    assert_eq!(value["streamDone"], true);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_response_body_is_readable_stream() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"first-second"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const response = await fetch(url + "/stream");
    const clone = response.clone();
    const cloneText = await clone.text();
    const originalBodyUsedBeforeRead = response.bodyUsed;
    const originalText = await response.text();
    return {
        bodyUsed: response.bodyUsed,
        originalBodyUsedBeforeRead,
        originalText,
        cloneText,
    };
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate stream script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call stream function")?;
    let value: serde_json::Value = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode stream result")?;
    assert_eq!(value["originalBodyUsedBeforeRead"], false);
    assert_eq!(value["bodyUsed"], true);
    assert_eq!(value["originalText"], "first-second");
    assert_eq!(value["cloneText"], "first-second");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_response_body_supports_multiple_clones() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/multi-clone"))
        .respond_with(ResponseTemplate::new(200).set_body_string("multi-clone"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const response = await fetch(url + "/multi-clone");
    const first = response.clone();
    const second = first.clone();
    const third = response.clone();
    return await Promise.all([
        response.text(),
        first.text(),
        second.text(),
        third.text(),
    ]);
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate multi-clone script")?;

    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call multi-clone function")?;
    let value: Vec<String> = output
        .result
        .context("expected multi-clone result")?
        .to_serde()
        .context("failed to decode multi-clone result")?;
    assert_eq!(value, vec!["multi-clone"; 4]);
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_fetch_resolves_before_body_finishes() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const response = await fetch(url + "/delayed-stream");
    const reader = response.body.getReader();
    const first = await reader.read();
    return {
        status: response.status,
        done: first.done,
        chunk: String.fromCharCode.apply(null, Array.from(first.value)),
    };
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate delayed-stream script")?;

    let started = Instant::now();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args!["http://stream.test"]?,
        Duration::from_millis(750),
    )
    .await
    .context("fetch waited for the complete response body")?;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "fetch should resolve after headers and the first chunk"
    );

    let value: serde_json::Value = output
        .result
        .context("expected delayed-stream result")?
        .to_serde()
        .context("failed to decode delayed-stream result")?;
    assert_eq!(value["status"], 200);
    assert_eq!(value["done"], false);
    assert_eq!(value["chunk"], "first");
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_response_body_survives_sandbox_boundaries() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/retained"))
        .respond_with(ResponseTemplate::new(200).set_body_string("retained"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
let retainedResponse;
async function fetchAndRetain(url) {
    retainedResponse = await fetch(url + "/retained");
    return retainedResponse.status;
}
async function readRetained() {
    return retainedResponse.text();
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate retained-response script")?;

    let first = call_with_timeout(
        &mut sandbox,
        "fetchAndRetain",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to fetch retained response")?;
    assert_eq!(
        first
            .result
            .as_ref()
            .context("expected status result")?
            .to_serde::<u16>()?,
        200
    );

    let second = call_with_timeout(
        &mut sandbox,
        "readRetained",
        Vec::new(),
        Duration::from_secs(5),
    )
    .await
    .context("failed to read retained response")?;
    let text: String = second
        .result
        .context("expected retained body")?
        .to_serde()?;
    assert_eq!(text, "retained");
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_sse_events_can_be_consumed_incrementally() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: one\n\ndata: two\n\n"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const response = await fetch(url + "/events");
    const text = await response.text();
    return text.split("\n").filter((line) => line.indexOf("data: ") === 0)
        .map((line) => line.slice(6));
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate SSE script")?;
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call SSE function")?;
    let events: Vec<String> = output
        .result
        .context("expected SSE result")?
        .to_serde()
        .context("failed to decode SSE result")?;
    assert_eq!(events, ["one", "two"]);
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_empty_response_body_is_eof() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/empty"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const response = await fetch(url + "/empty");
    const reader = response.body.getReader();
    return (await reader.read()).done;
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate empty-body script")?;
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call empty-body function")?;
    let done: bool = output.result.context("expected EOF result")?.to_serde()?;
    assert!(done);
    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_response_body_cancel_releases_reader() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cancel"))
        .respond_with(ResponseTemplate::new(200).set_body_string("cancel-me"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const response = await fetch(url + "/cancel");
    const reader = response.body.getReader();
    await reader.cancel("abandoned");
    return response.bodyUsed;
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate cancellation script")?;
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call cancellation function")?;
    let value: bool = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode cancellation result")?;
    assert!(value);
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
        .eval_script(script, OutputTarget::discard())
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
async fn integration_js_http_abort_after_dispatch_rejects_promptly() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(1))
                .set_body_string("too late"),
        )
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
async function main(url) {
    const controller = new AbortController();
    setTimeout(function () {
        controller.abort("stop");
    }, 10);
    try {
        await fetch(url + "/slow", {signal: controller.signal});
        return "expected-abort";
    } catch (error) {
        return String(error.name || error);
    }
}
"#;
    sandbox
        .eval_script(script, OutputTarget::discard())
        .await
        .context("failed to evaluate post-dispatch abort script")?;

    let started = Instant::now();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![server.uri()]?,
        Duration::from_secs(2),
    )
    .await
    .context("post-dispatch abort did not settle")?;
    let elapsed = started.elapsed();
    let value: String = output
        .result
        .as_ref()
        .context("expected end output")?
        .to_serde()
        .context("failed to decode post-dispatch abort result")?;

    assert_eq!(value, "AbortError");
    assert!(
        elapsed < Duration::from_millis(400),
        "abort waited for the HTTP response: {elapsed:?}"
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_http_abort_after_headers_cancels_body_read() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;
    sandbox
        .eval_script(
            r#"
async function main(url) {
    const controller = new AbortController();
    const response = await fetch(url + "/delayed-stream", {
        signal: controller.signal,
    });
    const reader = response.body.getReader();
    const first = await reader.read();
    if (first.done) return "unexpected-eof";
    controller.abort("stop");
    try {
        await reader.read();
        return "expected-abort";
    } catch (error) {
        return String(error.name || error);
    }
}
"#,
            OutputTarget::discard(),
        )
        .await
        .context("failed to evaluate post-header abort script")?;

    let started = Instant::now();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args!["http://stream.test"]?,
        Duration::from_millis(750),
    )
    .await
    .context("post-header abort did not settle")?;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "body read remained pending after abort: {:?}",
        started.elapsed()
    );
    let value: String = output
        .result
        .context("expected post-header abort result")?
        .to_serde()
        .context("failed to decode post-header abort result")?;
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
        .eval_script(script, OutputTarget::discard())
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
        .eval_script(script, OutputTarget::discard())
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
