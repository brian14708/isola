use std::{collections::BTreeMap, sync::Arc};

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
    threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;

fn io_error(msg: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(msg.into()))
}

#[napi(object)]
pub struct JsHttpResponse {
    pub status: u16,
    pub headers: Option<BTreeMap<String, String>>,
    pub body: Option<Buffer>,
    pub stream_handle: Option<u32>,
}

#[napi(object)]
pub struct JsHttpStreamChunk {
    pub body: Option<Buffer>,
    pub done: bool,
}

// ---------------------------------------------------------------------------
// Hostcall handler bridge
// ---------------------------------------------------------------------------

// The ThreadsafeFunction type built from Function<(String, String),
// Promise<String>>.build_threadsafe_function().build() Type params: T, Return,
// CallJsBackArgs, ErrorStatus, CalleeHandled
type HostcallTsfn =
    ThreadsafeFunction<(String, Buffer), Promise<Buffer>, (String, Buffer), Status, false>;

pub struct JsHostcallHandler {
    tsfn: HostcallTsfn,
}

impl JsHostcallHandler {
    pub(crate) const fn new(tsfn: HostcallTsfn) -> Self {
        Self { tsfn }
    }

    pub(crate) async fn invoke(
        &self,
        call_type: &str,
        payload: Value,
    ) -> std::result::Result<Value, BoxError> {
        let payload_cbor = payload.into_cbor();

        let promise = self
            .tsfn
            .call_async((call_type.to_owned(), Buffer::from(payload_cbor.to_vec())))
            .await
            .map_err(|e| io_error(format!("hostcall JS handler failed: {e}")))?;

        let result_cbor = promise
            .await
            .map_err(|e| io_error(format!("hostcall JS promise rejected: {e}")))?;

        Ok(Value::from_cbor(result_cbor.to_vec()))
    }
}

// ---------------------------------------------------------------------------
// HTTP handler bridge
// ---------------------------------------------------------------------------

type HttpTsfn = ThreadsafeFunction<
    (String, String, Buffer, Option<Buffer>),
    Promise<JsHttpResponse>,
    (String, String, Buffer, Option<Buffer>),
    Status,
    false,
>;
type HttpStreamTsfn = ThreadsafeFunction<
    (String, String, Buffer, Option<Buffer>),
    Promise<JsHttpStreamChunk>,
    (String, String, Buffer, Option<Buffer>),
    Status,
    false,
>;

struct HttpStreamState {
    tsfn: Arc<HttpStreamTsfn>,
    handle: u32,
}

impl Drop for HttpStreamState {
    fn drop(&mut self) {
        let _ = self.tsfn.call(
            (
                "__isola_stream_release".to_owned(),
                self.handle.to_string(),
                Buffer::from(Vec::new()),
                None,
            ),
            ThreadsafeFunctionCallMode::NonBlocking,
        );
    }
}
pub struct JsHttpHandler {
    tsfn: Arc<HttpTsfn>,
    stream_tsfn: Arc<HttpStreamTsfn>,
}

impl JsHttpHandler {
    pub(crate) fn new(tsfn: HttpTsfn, stream_tsfn: HttpStreamTsfn) -> Self {
        Self {
            tsfn: Arc::new(tsfn),
            stream_tsfn: Arc::new(stream_tsfn),
        }
    }

    pub(crate) async fn invoke(
        &self,
        incoming: HttpRequest,
    ) -> std::result::Result<HttpResponse, BoxError> {
        let method = incoming.method().as_str().to_owned();
        let url = incoming.uri().to_string();

        let headers: BTreeMap<String, String> = incoming
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|val| (k.as_str().to_string(), val.to_string()))
            })
            .collect();
        let headers_json = serde_json::to_string(&headers)
            .map_err(|e| io_error(format!("failed to serialize headers: {e}")))?;

        let body = incoming.body().as_ref().map(|b| Buffer::from(b.to_vec()));

        let promise = self
            .tsfn
            .call_async((method, url, Buffer::from(headers_json.into_bytes()), body))
            .await
            .map_err(|e| io_error(format!("HTTP JS handler failed: {e}")))?;

        let resp = promise
            .await
            .map_err(|e| io_error(format!("HTTP JS promise rejected: {e}")))?;

        let mut builder = http::Response::builder().status(resp.status);

        if let Some(headers) = resp.headers {
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
        }

        let body_stream: HttpBodyStream = if let Some(handle) = resp.stream_handle {
            let tsfn = self.stream_tsfn.clone();
            Box::pin(stream::unfold(
                HttpStreamState { tsfn, handle },
                |state| async move {
                    let tsfn = state.tsfn.clone();
                    let handle = state.handle;
                    let result = match tsfn
                        .call_async((
                            "__isola_stream_read".to_owned(),
                            handle.to_string(),
                            Buffer::from(Vec::new()),
                            None,
                        ))
                        .await
                    {
                        Ok(promise) => promise
                            .await
                            .map_err(|e| io_error(format!("HTTP stream promise rejected: {e}"))),
                        Err(e) => Err(io_error(format!("HTTP stream callback failed: {e}"))),
                    };
                    match result {
                        Ok(resp) => {
                            let done = resp.done;
                            let item = resp.body.map_or_else(
                                || Ok(Frame::data(Bytes::new())),
                                |body| Ok(Frame::data(Bytes::from(Vec::<u8>::from(body)))),
                            );
                            if done { None } else { Some((item, state)) }
                        }
                        Err(error) => Some((Err(error), state)),
                    }
                },
            ))
        } else if let Some(body) = resp.body {
            let body_bytes = Bytes::from(Vec::<u8>::from(body));
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
