use std::{collections::BTreeMap, sync::Arc};

use bytes::Bytes;
use futures::stream;
use http_body::Frame;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequestStream, HttpResponse},
    value::Value,
};
use napi::{
    Status,
    bindgen_prelude::{Buffer, Promise},
    threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;

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

struct JsRequestStream {
    source: AsyncMutex<HttpBodyStream>,
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
    (String, String, Buffer, u32),
    Promise<JsHttpResponse>,
    (String, String, Buffer, u32),
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
    request_streams: Arc<Mutex<BTreeMap<u32, Arc<JsRequestStream>>>>,
    next_request_stream: std::sync::atomic::AtomicU32,
}

impl JsHttpHandler {
    pub(crate) fn new(tsfn: HttpTsfn, stream_tsfn: HttpStreamTsfn) -> Self {
        Self {
            tsfn: Arc::new(tsfn),
            stream_tsfn: Arc::new(stream_tsfn),
            request_streams: Arc::new(Mutex::new(BTreeMap::new())),
            next_request_stream: std::sync::atomic::AtomicU32::new(1),
        }
    }

    fn register_request_stream(&self, body: HttpBodyStream) -> u32 {
        let handle = self
            .next_request_stream
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.request_streams.lock().insert(
            handle,
            Arc::new(JsRequestStream {
                source: AsyncMutex::new(body),
            }),
        );
        handle
    }

    pub(crate) async fn read_request_stream(
        &self,
        handle: u32,
    ) -> std::result::Result<JsHttpStreamChunk, BoxError> {
        let Some(stream) = self.request_streams.lock().get(&handle).cloned() else {
            return Ok(JsHttpStreamChunk {
                body: None,
                done: true,
            });
        };
        let mut source = stream.source.lock().await;
        let item = futures::StreamExt::next(&mut *source).await;
        drop(source);
        match item {
            None => {
                self.request_streams.lock().remove(&handle);
                Ok(JsHttpStreamChunk {
                    body: None,
                    done: true,
                })
            }
            Some(Ok(frame)) => frame.into_data().map_or_else(
                |_| {
                    Ok(JsHttpStreamChunk {
                        body: None,
                        done: false,
                    })
                },
                |data| {
                    Ok(JsHttpStreamChunk {
                        body: Some(Buffer::from(data.to_vec())),
                        done: false,
                    })
                },
            ),
            Some(Err(error)) => {
                self.request_streams.lock().remove(&handle);
                Err(error)
            }
        }
    }

    pub(crate) fn release_request_stream(&self, handle: u32) {
        self.request_streams.lock().remove(&handle);
    }

    pub(crate) async fn invoke(
        &self,
        incoming: HttpRequestStream,
    ) -> std::result::Result<HttpResponse, BoxError> {
        let (parts, body) = incoming.into_parts();
        let method = parts.method.as_str().to_owned();
        let url = parts.uri.to_string();
        let headers: BTreeMap<String, String> = parts
            .headers
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|value| (k.as_str().to_owned(), value.to_owned()))
            })
            .collect();
        let headers_json = serde_json::to_string(&headers)
            .map_err(|e| io_error(format!("failed to serialize headers: {e}")))?;
        let stream_handle = self.register_request_stream(body);
        let promise = self
            .tsfn
            .call_async((
                method,
                url,
                Buffer::from(headers_json.into_bytes()),
                stream_handle,
            ))
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
                    let result = match state
                        .tsfn
                        .call_async((
                            "__isola_stream_read".to_owned(),
                            state.handle.to_string(),
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
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => return Some((Err(error), state)),
                    };
                    if result.done {
                        return None;
                    }
                    let item = result.body.map_or_else(
                        || Ok(Frame::data(Bytes::new())),
                        |body| Ok(Frame::data(Bytes::from(Vec::<u8>::from(body)))),
                    );
                    Some((item, state))
                },
            ))
        } else if let Some(body) = resp.body {
            Box::pin(stream::once(async move {
                Ok(Frame::data(Bytes::from(Vec::<u8>::from(body))))
            }))
        } else {
            Box::pin(stream::empty())
        };
        builder
            .body(body_stream)
            .map_err(|e| io_error(format!("invalid response metadata: {e}")))
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

    async fn http_request_stream(
        &self,
        incoming: HttpRequestStream,
    ) -> std::result::Result<HttpResponse, BoxError> {
        let handler = self
            .http_handler
            .as_ref()
            .ok_or_else(|| io_error("unsupported http_request"))?;
        handler.invoke(incoming).await
    }
}
