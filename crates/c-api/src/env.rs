use std::{
    ffi::{CString, c_char},
    sync::{Arc, Mutex, OnceLock},
};

use async_trait::async_trait;
use bytes::Bytes;
use http_body::Frame;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse},
    value::Value,
};
use tokio_stream::wrappers::ReceiverStream;

use crate::error::ErrorCode;

/// C-compatible HTTP header.
#[repr(C)]
pub struct HttpHeader {
    pub name: *const u8,
    pub name_len: usize,
    pub value: *const u8,
    pub value_len: usize,
}

/// C-compatible HTTP request passed to the handler callback.
#[repr(C)]
pub struct HttpRequestInfo {
    pub method: *const c_char,
    pub url: *const c_char,
    pub headers: *const HttpHeader,
    pub headers_len: usize,
    pub body: *const u8,
    pub body_len: usize,
}

/// Status + headers delivered by the C side via `start`.
pub struct HttpResponseHead {
    pub status: u16,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Opaque handle for an in-flight HTTP response.
///
/// The C side drives the response through three phases:
/// 1. `isola_http_response_body_start` — deliver status and headers
/// 2. `isola_http_response_body_push` — deliver body chunks (zero or more)
/// 3. `isola_http_response_body_close` — signal EOF and free the handle
pub struct HttpResponseBody {
    head: Mutex<Option<tokio::sync::oneshot::Sender<HttpResponseHead>>>,
    body: tokio::sync::mpsc::Sender<Result<Frame<Bytes>, BoxError>>,
}

impl HttpResponseBody {
    /// Send the HTTP status code and response headers. Must be called exactly
    /// once before pushing body data. Returns `Err` if already called.
    pub fn start(&self, head: HttpResponseHead) -> Result<(), ()> {
        self.head
            .lock()
            .unwrap()
            .take()
            .ok_or(())?
            .send(head)
            .map_err(|_| ())
    }

    /// Push a body data frame. Blocks the calling thread if the channel is
    /// full. Returns `Err` if the receiver has been dropped.
    pub fn send(&self, data: Bytes) -> Result<(), ()> {
        self.body
            .blocking_send(Ok(Frame::data(data)))
            .map_err(|_| ())
    }
}

#[derive(Clone)]
pub struct Env {
    handler: Arc<OnceLock<Arc<crate::SandboxHandler>>>,
}

impl Env {
    pub const fn new(handler: Arc<OnceLock<Arc<crate::SandboxHandler>>>) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl Host for Env {
    async fn hostcall(&self, call_type: &str, payload: Value) -> Result<Value, BoxError> {
        match call_type {
            "echo" => Ok(payload),
            _ => Err(
                std::io::Error::new(std::io::ErrorKind::Unsupported, "unknown hostcall type")
                    .into(),
            ),
        }
    }

    async fn http_request(&self, incoming: HttpRequest) -> Result<HttpResponse, BoxError> {
        let handler = self
            .handler
            .get()
            .ok_or_else(|| -> BoxError {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "handler not set",
                ))
            })?
            .clone();

        let http_request_fn = handler.vtable.http_request.ok_or_else(|| -> BoxError {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no HTTP handler registered",
            ))
        })?;

        // Create channels. Rust keeps the receivers; C gets the senders
        // wrapped in an HttpResponseBody handle.
        let (head_tx, head_rx) = tokio::sync::oneshot::channel();
        let (body_tx, body_rx) = tokio::sync::mpsc::channel(32);

        // Inner block ensures all raw-pointer locals (c_request, etc.) are
        // dropped before the `.await`, keeping the future `Send`.
        {
            let header_pairs: Vec<(Vec<u8>, Vec<u8>)> = incoming
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect();

            let c_headers: Vec<HttpHeader> = header_pairs
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: name.as_ptr(),
                    name_len: name.len(),
                    value: value.as_ptr(),
                    value_len: value.len(),
                })
                .collect();

            let method = CString::new(incoming.method().as_str())
                .map_err(|e| -> BoxError { Box::new(std::io::Error::other(e)) })?;
            let url = CString::new(incoming.uri().to_string())
                .map_err(|e| -> BoxError { Box::new(std::io::Error::other(e)) })?;

            let body_bytes = incoming.body().as_ref().map(|b| b.to_vec());
            let (body_ptr, body_len) = body_bytes
                .as_ref()
                .map_or((std::ptr::null(), 0), |b| (b.as_ptr(), b.len()));

            let c_request = HttpRequestInfo {
                method: method.as_ptr(),
                url: url.as_ptr(),
                headers: if c_headers.is_empty() {
                    std::ptr::null()
                } else {
                    c_headers.as_ptr()
                },
                headers_len: c_headers.len(),
                body: body_ptr,
                body_len,
            };

            let response_body = Box::into_raw(Box::new(HttpResponseBody {
                head: Mutex::new(Some(head_tx)),
                body: body_tx,
            }));

            let code = http_request_fn(&raw const c_request, response_body, handler.user_data);

            if code != ErrorCode::Ok {
                drop(unsafe { Box::from_raw(response_body) });
                return Err(Box::new(std::io::Error::other(
                    "HTTP handler callback failed",
                )));
            }
        }

        // Await status + headers from C side (non-blocking on the async runtime).
        let head = head_rx.await.map_err(|_| -> BoxError {
            Box::new(std::io::Error::other("HTTP response closed without status"))
        })?;

        let body_stream: HttpBodyStream = Box::pin(ReceiverStream::new(body_rx));

        let mut builder = http::Response::builder().status(head.status);
        for (name, value) in &head.headers {
            builder = builder.header(name.as_slice(), value.as_slice());
        }
        let response = builder
            .body(body_stream)
            .map_err(|e| -> BoxError { Box::new(e) })?;

        Ok(response)
    }
}
