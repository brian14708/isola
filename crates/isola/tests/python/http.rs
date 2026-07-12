use std::time::Duration;

use anyhow::{Context, Result};
use isola::{
    host::NoopOutputSink,
    sandbox::{Arg, CallOutput, Sandbox, SandboxOptions, args},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, body_string_contains, header, header_regex, method, path},
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
async fn integration_python_http_client_roundtrip() -> Result<()> {
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
from sandbox.http import fetch

def main(url):
    with fetch(
        "POST",
        url,
        headers={"content-type": "application/json"},
        body=b'{"hello":"world"}',
    ) as resp:
        return resp.text()
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate http fetch script")?;

    let url_arg = format!("{}/echo", server.uri());
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
async fn integration_python_http_large_response_is_chunked_and_limited() -> Result<()> {
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
from sandbox.http import fetch

def main(url):
    with fetch("GET", f"{url}/large") as resp:
        body = resp.read()

    try:
        with fetch("GET", f"{url}/oversized") as resp:
            resp.read()
        oversized_error = "expected response-size error"
    except Exception as e:
        oversized_error = str(e)

    return (len(body), body[0], body[-1], oversized_error)
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
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

/// A zero-length `read(0)` must return promptly (not spin on the always-ready
/// pollable) and must leave the response readable for a subsequent full read.
#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_http_zero_length_read_does_not_hang() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/body"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello world"))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
from sandbox.http import fetch

def main(url):
    with fetch("GET", url) as resp:
        resp.read(0)        # must not hang
        return resp.text()  # response still readable afterwards
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate zero-length read script")?;

    let url_arg = format!("{}/body", server.uri());
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("zero-length read hung or failed")?;

    let value: String = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode response body")?;
    assert_eq!(value, "hello world");

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_http_status_errors_surface() -> Result<()> {
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
from sandbox.http import fetch

def main(url):
    with fetch("GET", f"{url}/status/503") as first:
        first_status = first.status

    with fetch("POST", f"{url}/status/500", body={"value": "test"}) as second:
        second_status = second.status

    return (first_status, second_status)
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
async fn integration_python_http_multipart_files() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/multipart"))
        .and(header_regex(
            "content-type",
            r"^multipart/form-data;\s*boundary=.*$",
        ))
        .and(body_string_contains(r#"name="file"; filename="file""#))
        .and(body_string_contains("\r\n\r\ntest\r\n"))
        .and(body_string_contains(r#"name="file2"; filename="a.txt""#))
        .and(body_string_contains("Content-Type: text/plain"))
        .and(body_string_contains("\r\n\r\ntest2\r\n"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
import io
from sandbox.http import fetch

def main(url):
    with fetch(
        "POST",
        f"{url}/multipart",
        files={
            "file": b"test",
            "file2": ("a.txt", io.BytesIO(b"test2"), "text/plain"),
        },
    ) as resp:
        return resp.status
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate multipart script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call multipart function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: i64 = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode multipart status")?;
    assert_eq!(value, 200);

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_http_read_twice_errors() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/read-twice"))
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
from sandbox.http import fetch

def main(url):
    with fetch("GET", f"{url}/read-twice") as resp:
        _ = resp.json()
        try:
            _ = resp.json()
            return "expected-second-read-error"
        except Exception as e:
            return str(e)
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate read-twice script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call read-twice function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: String = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode read-twice result")?;
    assert!(
        value.contains("Response already read"),
        "unexpected second-read error message: {value}"
    );

    Ok(())
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_python_http_timeout_is_enforced() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(200))
                .set_body_string("too late"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut sandbox = module
        .instantiate(TestHost::default(), SandboxOptions::default())
        .await
        .context("failed to instantiate sandbox")?;

    let script = r#"
from sandbox.http import fetch

def main(url):
    try:
        with fetch("GET", f"{url}/slow", timeout=0.05) as resp:
            return resp.text()
    except Exception as e:
        return str(e)
"#;
    sandbox
        .eval_script(script, NoopOutputSink::shared())
        .await
        .context("failed to evaluate timeout script")?;

    let url_arg = server.uri();
    let output = call_with_timeout(
        &mut sandbox,
        "main",
        args![url_arg]?,
        Duration::from_secs(5),
    )
    .await
    .context("failed to call timeout function")?;

    assert!(output.items.is_empty(), "expected no partial outputs");
    let value: String = output
        .result
        .as_ref()
        .context("expected exactly one end output")?
        .to_serde()
        .context("failed to decode timeout result")?;
    assert!(
        value.contains("timed out"),
        "unexpected timeout error message: {value}"
    );

    Ok(())
}
