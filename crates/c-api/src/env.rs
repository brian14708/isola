use std::{
    ffi::{CString, c_char},
    sync::{Arc, OnceLock},
};

use bytes::Bytes;
use http_body::Frame;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequestStream, HttpResponse},
    value::Value,
};
use parking_lot::Mutex;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// C-compatible HTTP header.
#[repr(C)]
pub struct HttpHeader {
    pub name: *const u8,
    pub name_len: usize,
    pub value: *const u8,
    pub value_len: usize,
}

/// C-compatible HTTP request passed to the handler callback.
///
/// All referenced request data is valid only for the duration of the callback.
/// Copy any values needed by asynchronous work.
#[repr(C)]
pub struct HttpRequestInfo {
    pub method: *const c_char,
    pub url: *const c_char,
    pub headers: *const HttpHeader,
    pub headers_len: usize,
    /// Streaming request body. The callback owns this handle and must close
    /// it after consuming or abandoning the request body.
    pub body_stream: *mut HttpRequestBody,
}

type HttpRequestChunk = Result<Frame<Bytes>, BoxError>;
type HttpRequestReceiver = std::sync::mpsc::Receiver<HttpRequestChunk>;

/// Opaque handle for a streaming HTTP request body.
///
/// The C side reads request chunks with `isola_http_request_body_read`. The
/// read call blocks until the next chunk is available, which preserves
/// backpressure from the guest runtime. The data pointer returned by a read is
/// valid until the next read or until the handle is closed.
pub struct HttpRequestBody {
    receiver: Mutex<Option<HttpRequestReceiver>>,
    chunk: Mutex<Option<Bytes>>,
}

impl HttpRequestBody {
    pub fn new(receiver: HttpRequestReceiver) -> Self {
        Self {
            receiver: Mutex::new(Some(receiver)),
            chunk: Mutex::new(None),
        }
    }

    /// Read the next data frame, blocking until data or EOF is available.
    ///
    /// The returned pointer borrows the internally-stashed chunk and stays
    /// valid until the next call or until the handle is closed.
    pub fn read_chunk(&self) -> Result<Option<(*const u8, usize)>, String> {
        let chunk = self.read()?;
        let mut stashed = self.chunk.lock();
        *stashed = chunk;
        Ok(stashed.as_ref().map(|chunk| (chunk.as_ptr(), chunk.len())))
    }

    fn read(&self) -> Result<Option<Bytes>, String> {
        let mut receiver_slot = self.receiver.lock();
        let Some(receiver) = receiver_slot.as_mut() else {
            return Ok(None);
        };

        loop {
            match receiver.recv() {
                Ok(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        return Ok(Some(data));
                    }
                }
                Ok(Err(error)) => {
                    return Err(format!("HTTP request body stream failed: {error}"));
                }
                Err(_) => {
                    *receiver_slot = None;
                    drop(receiver_slot);
                    return Ok(None);
                }
            }
        }
    }
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
            .take()
            .ok_or(())?
            .send(head)
            .map_err(|_| ())
    }

    /// Push a body data frame. Blocks the calling thread if the channel is
    /// full. Returns `Err` if the receiver has been dropped.
    pub fn send(&self, data: Bytes) -> Result<(), ()> {
        if self.head.lock().is_some() {
            return Err(());
        }
        self.body
            .blocking_send(Ok(Frame::data(data)))
            .map_err(|_| ())
    }
}

/// Opaque handle for an in-flight hostcall response.
///
/// The C side delivers the result by calling exactly one of:
/// - `isola_hostcall_response_resolve` — on success
/// - `isola_hostcall_response_reject` — on failure
///
/// Either call consumes and frees the handle.
pub struct HostcallResponse {
    sender: Mutex<Option<tokio::sync::oneshot::Sender<Result<Value, BoxError>>>>,
}

impl HostcallResponse {
    /// Resolve the hostcall with a successful value.
    pub fn resolve(self, value: Value) -> Result<(), ()> {
        self.sender
            .into_inner()
            .ok_or(())?
            .send(Ok(value))
            .map_err(|_| ())
    }

    /// Reject the hostcall with an error message.
    pub fn reject(self, error: String) -> Result<(), ()> {
        self.sender
            .into_inner()
            .ok_or(())?
            .send(Err(Box::new(std::io::Error::other(error))))
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

impl Host for Env {
    async fn hostcall(&self, call_type: &str, payload: Value) -> Result<Value, BoxError> {
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

        let hostcall_fn = handler.vtable.hostcall.ok_or_else(|| -> BoxError {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no hostcall handler registered",
            ))
        })?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        // Block scope: raw pointers do not cross the .await
        {
            let call_type_c = CString::new(call_type)
                .map_err(|e| -> BoxError { Box::new(std::io::Error::other(e)) })?;

            let payload_json = payload.to_json_str().map_err(|e| -> BoxError {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;

            let response = Box::into_raw(Box::new(HostcallResponse {
                sender: Mutex::new(Some(tx)),
            }));

            hostcall_fn(
                call_type_c.as_ptr(),
                payload_json.as_ptr(),
                payload_json.len(),
                response,
                handler.user_data,
            );
        }

        // Await response from C side
        rx.await.map_err(|_| -> BoxError {
            Box::new(std::io::Error::other(
                "hostcall response handle dropped without resolve/reject",
            ))
        })?
    }

    async fn http_request_stream(
        &self,
        incoming: HttpRequestStream,
    ) -> Result<HttpResponse, BoxError> {
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

        let (parts, mut incoming_body) = incoming.into_parts();
        let (body_tx, mut body_rx) = tokio::sync::mpsc::channel(32);
        let (c_body_tx, c_body_rx) = std::sync::mpsc::sync_channel(32);
        let request_body = Box::into_raw(Box::new(HttpRequestBody::new(c_body_rx)));

        // Keep blocking C reads off the async runtime using Tokio's bounded
        // blocking pool, while retaining a bounded queue between the guest
        // stream and the C callback.
        tokio::task::spawn_blocking(move || {
            while let Some(frame) = body_rx.blocking_recv() {
                if c_body_tx.send(frame).is_err() {
                    break;
                }
            }
        });

        // The C side pulls from this bounded channel. A full channel suspends
        // the pump, propagating backpressure into the guest request stream.
        tokio::spawn(async move {
            while let Some(frame) = incoming_body.next().await {
                if body_tx.send(frame).await.is_err() {
                    break;
                }
            }
        });

        let (head_tx, head_rx) = tokio::sync::oneshot::channel();
        let (response_tx, response_rx) = tokio::sync::mpsc::channel(32);

        // Inner block ensures raw-pointer locals do not cross the `.await`,
        // keeping the host future `Send`.
        {
            let header_pairs: Vec<(Vec<u8>, Vec<u8>)> = parts
                .headers
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

            let method = CString::new(parts.method.as_str())
                .map_err(|e| -> BoxError { Box::new(std::io::Error::other(e)) })?;
            let url = CString::new(parts.uri.to_string())
                .map_err(|e| -> BoxError { Box::new(std::io::Error::other(e)) })?;
            let c_request = HttpRequestInfo {
                method: method.as_ptr(),
                url: url.as_ptr(),
                headers: if c_headers.is_empty() {
                    std::ptr::null()
                } else {
                    c_headers.as_ptr()
                },
                headers_len: c_headers.len(),
                body_stream: request_body,
            };

            let response_body = Box::into_raw(Box::new(HttpResponseBody {
                head: Mutex::new(Some(head_tx)),
                body: response_tx,
            }));

            http_request_fn(&raw const c_request, response_body, handler.user_data);
        }

        let head = head_rx.await.map_err(|_| -> BoxError {
            Box::new(std::io::Error::other("HTTP response closed without status"))
        })?;

        let body_stream: HttpBodyStream = Box::pin(ReceiverStream::new(response_rx));
        let mut builder = http::Response::builder().status(head.status);
        for (name, value) in &head.headers {
            builder = builder.header(name.as_slice(), value.as_slice());
        }
        builder
            .body(body_stream)
            .map_err(|e| -> BoxError { Box::new(e) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_body_rejects_data_before_response_head() {
        let (head_tx, _head_rx) = tokio::sync::oneshot::channel();
        let (body_tx, _body_rx) = tokio::sync::mpsc::channel(1);
        let body = HttpResponseBody {
            head: Mutex::new(Some(head_tx)),
            body: body_tx,
        };

        assert!(body.send(Bytes::from_static(b"early")).is_err());
        assert!(
            body.start(HttpResponseHead {
                status: 200,
                headers: Vec::new(),
            })
            .is_ok()
        );
        assert!(body.send(Bytes::from_static(b"ready")).is_ok());
    }
}
