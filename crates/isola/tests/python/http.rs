use std::time::Duration;

use anyhow::{Context, Result};
use isola::cbor::from_cbor;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, body_string_contains, header, header_regex, method, path},
};

use super::common::{TestHost, build_module, call_collect, cbor_arg};

#[tokio::test]
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
        .instantiate(None, TestHost::default())
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
        .eval_script(script)
        .await
        .context("failed to evaluate http fetch script")?;

    let url_arg = serde_json::to_string(&format!("{}/echo", server.uri()))
        .context("failed to encode mock server URL")?;
    let state = call_collect(
        &mut sandbox,
        "main",
        vec![cbor_arg(None, &url_arg)?],
        Duration::from_secs(5),
    )
    .await
    .context("failed to call http fetch function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");

    let value: String =
        from_cbor(state.end[0].as_ref()).context("failed to decode response body")?;
    assert_eq!(value, r#"{"ok":true}"#);

    Ok(())
}

#[tokio::test]
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
        .instantiate(None, TestHost::default())
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
        .eval_script(script)
        .await
        .context("failed to evaluate status script")?;

    let url_arg =
        serde_json::to_string(&server.uri()).context("failed to encode mock server URL")?;
    let state = call_collect(
        &mut sandbox,
        "main",
        vec![cbor_arg(None, &url_arg)?],
        Duration::from_secs(5),
    )
    .await
    .context("failed to call status function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: (i64, i64) =
        from_cbor(state.end[0].as_ref()).context("failed to decode status tuple")?;
    assert_eq!(value, (503, 500));

    Ok(())
}

#[tokio::test]
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
        .instantiate(None, TestHost::default())
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
        .eval_script(script)
        .await
        .context("failed to evaluate multipart script")?;

    let url_arg =
        serde_json::to_string(&server.uri()).context("failed to encode mock server URL")?;
    let state = call_collect(
        &mut sandbox,
        "main",
        vec![cbor_arg(None, &url_arg)?],
        Duration::from_secs(5),
    )
    .await
    .context("failed to call multipart function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: i64 =
        from_cbor(state.end[0].as_ref()).context("failed to decode multipart status")?;
    assert_eq!(value, 200);

    Ok(())
}

#[tokio::test]
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
        .instantiate(None, TestHost::default())
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
        .eval_script(script)
        .await
        .context("failed to evaluate read-twice script")?;

    let url_arg =
        serde_json::to_string(&server.uri()).context("failed to encode mock server URL")?;
    let state = call_collect(
        &mut sandbox,
        "main",
        vec![cbor_arg(None, &url_arg)?],
        Duration::from_secs(5),
    )
    .await
    .context("failed to call read-twice function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: String =
        from_cbor(state.end[0].as_ref()).context("failed to decode read-twice result")?;
    assert!(
        value.contains("Response already read"),
        "unexpected second-read error message: {value}"
    );

    Ok(())
}
