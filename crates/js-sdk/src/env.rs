use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use http_body::Frame;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse},
    value::Value,
};
use napi::{
    Status,
    bindgen_prelude::{Buffer, Promise},
    threadsafe_function::ThreadsafeFunction,
};

fn io_error(msg: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(msg.into()))
}

// ---------------------------------------------------------------------------
// Hostcall handler bridge
// ---------------------------------------------------------------------------

// The ThreadsafeFunction type built from Function<(String, String),
// Promise<String>>.build_threadsafe_function().build() Type params: T, Return,
// CallJsBackArgs, ErrorStatus, CalleeHandled
type HostcallTsfn =
    ThreadsafeFunction<(String, String), Promise<String>, (String, String), Status, false>;

pub struct JsHostcallHandler {
    tsfn: HostcallTsfn,
}

// SAFETY: ThreadsafeFunction is designed for cross-thread use.
unsafe impl Send for JsHostcallHandler {}
unsafe impl Sync for JsHostcallHandler {}

impl JsHostcallHandler {
    pub(crate) const fn new(tsfn: HostcallTsfn) -> Self {
        Self { tsfn }
    }

    pub(crate) async fn invoke(
        &self,
        call_type: &str,
        payload: Value,
    ) -> std::result::Result<Value, BoxError> {
        let payload_json = payload
            .to_json_str()
            .map_err(|e| io_error(format!("failed to encode hostcall payload: {e}")))?;

        let promise = self
            .tsfn
            .call_async((call_type.to_owned(), payload_json))
            .await
            .map_err(|e| io_error(format!("hostcall JS handler failed: {e}")))?;

        let result_json = promise
            .await
            .map_err(|e| io_error(format!("hostcall JS promise rejected: {e}")))?;

        Value::from_json_str(&result_json)
            .map_err(|e| io_error(format!("invalid hostcall response JSON: {e}")))
    }
}

// ---------------------------------------------------------------------------
// HTTP handler bridge
// ---------------------------------------------------------------------------

type HttpTsfn = ThreadsafeFunction<
    (String, String, String, Option<Buffer>),
    Promise<String>,
    (String, String, String, Option<Buffer>),
    Status,
    false,
>;

pub struct JsHttpHandler {
    tsfn: HttpTsfn,
}

// SAFETY: ThreadsafeFunction is designed for cross-thread use.
unsafe impl Send for JsHttpHandler {}
unsafe impl Sync for JsHttpHandler {}

impl JsHttpHandler {
    pub(crate) const fn new(tsfn: HttpTsfn) -> Self {
        Self { tsfn }
    }

    pub(crate) async fn invoke(
        &self,
        incoming: HttpRequest,
    ) -> std::result::Result<HttpResponse, BoxError> {
        let method = incoming.method().as_str().to_owned();
        let url = incoming.uri().to_string();

        let headers: serde_json::Value = incoming
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str().ok().map(|val| {
                    (
                        k.as_str().to_string(),
                        serde_json::Value::String(val.to_string()),
                    )
                })
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        let headers_json = serde_json::to_string(&headers)
            .map_err(|e| io_error(format!("failed to serialize headers: {e}")))?;

        let body = incoming.body().as_ref().map(|b| Buffer::from(b.to_vec()));

        let promise = self
            .tsfn
            .call_async((method, url, headers_json, body))
            .await
            .map_err(|e| io_error(format!("HTTP JS handler failed: {e}")))?;

        let result_json = promise
            .await
            .map_err(|e| io_error(format!("HTTP JS promise rejected: {e}")))?;

        let resp: serde_json::Value = serde_json::from_str(&result_json)
            .map_err(|e| io_error(format!("invalid HTTP response JSON: {e}")))?;

        let status = resp["status"]
            .as_u64()
            .ok_or_else(|| io_error("HTTP response missing status"))?;
        let status = u16::try_from(status).map_err(|_| io_error("HTTP status out of range"))?;

        let mut builder = http::Response::builder().status(status);

        if let Some(headers_obj) = resp["headers"].as_object() {
            for (k, v) in headers_obj {
                if let Some(val) = v.as_str() {
                    builder = builder.header(k.as_str(), val);
                }
            }
        }

        let body_stream: HttpBodyStream = if let Some(body_b64) = resp["body"].as_str() {
            let body_bytes = Bytes::from(body_b64.to_owned().into_bytes());
            Box::pin(stream::once(async move { Ok(Frame::data(body_bytes)) }))
        } else {
            Box::pin(stream::empty())
        };

        let response = builder
            .body(body_stream)
            .map_err(|e| io_error(format!("invalid response metadata: {e}")))?;

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Env: Host implementation
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Env {
    pub(crate) http_handler: Option<Arc<JsHttpHandler>>,
    pub(crate) hostcall_handler: Option<Arc<JsHostcallHandler>>,
}

impl Env {
    pub(crate) const fn new(
        http_handler: Option<Arc<JsHttpHandler>>,
        hostcall_handler: Option<Arc<JsHostcallHandler>>,
    ) -> Self {
        Self {
            http_handler,
            hostcall_handler,
        }
    }
}

#[async_trait]
impl Host for Env {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Value,
    ) -> std::result::Result<Value, BoxError> {
        let handler = self
            .hostcall_handler
            .as_ref()
            .ok_or_else(|| io_error(format!("unsupported hostcall: {call_type}")))?;
        handler.invoke(call_type, payload).await
    }

    async fn http_request(
        &self,
        incoming: HttpRequest,
    ) -> std::result::Result<HttpResponse, BoxError> {
        let handler = self
            .http_handler
            .as_ref()
            .ok_or_else(|| io_error("unsupported http_request"))?;
        handler.invoke(incoming).await
    }
}
