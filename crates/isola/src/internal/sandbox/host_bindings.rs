use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::BytesMut;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant as TokioInstant, timeout_at};
use tokio_stream::Stream;
use tracing::Instrument;
use wasmtime::component::{Accessor, Resource};

use super::{
    EmitValue, HostImpl, HostView, LinkerHost,
    isola::script::host::{
        EmitType, Host, HostValueIterator, HostValueIteratorWithStore, HostWithStore,
    },
};
use crate::{host::Host as _, value::Value};

const HTTP_HOSTCALL_TYPE: &str = "__isola_http";
const MAX_HTTP_HOSTCALL_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct HttpHostcallRequest {
    method: String,
    url: String,
    headers: Vec<(String, Vec<u8>)>,
    body: Option<Vec<u8>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct HttpHostcallResponse {
    status: u16,
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
}

pub struct ValueIterator {
    stream: Pin<Box<dyn Stream<Item = Value> + Send>>,
}

impl ValueIterator {
    #[must_use]
    pub fn new(stream: Pin<Box<dyn Stream<Item = Value> + Send>>) -> Self {
        Self { stream }
    }
}

impl<T: HostView> Host for HostImpl<T> {
    async fn blocking_emit(&mut self, emit_type: EmitType, cbor: Vec<u8>) -> wasmtime::Result<()> {
        let emit_value = match emit_type {
            EmitType::Continuation => EmitValue::Continuation(cbor.into()),
            EmitType::End => EmitValue::End(cbor.into()),
            EmitType::PartialResult => EmitValue::PartialResult(cbor.into()),
        };
        self.0.emit(emit_value).await
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "WIT-generated host traits are clearer as async methods even when some return immediately"
)]
impl<T: HostView> HostValueIterator for HostImpl<T> {
    async fn drop(&mut self, rep: Resource<ValueIterator>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

impl<T: HostView + 'static> HostValueIteratorWithStore<T> for LinkerHost<T> {
    async fn read(
        accessor: &Accessor<T, Self>,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Option<Vec<u8>>> {
        // Take the stream out (leaving an inert placeholder) so we can await
        // without holding the store across the await point. The resource stays
        // resident in the table, so its rep is preserved by construction rather
        // than relying on ResourceTable slot-reuse ordering.
        let mut stream = accessor.with(|mut access| -> wasmtime::Result<_> {
            let iter = access.get().0.table().get_mut(&resource)?;
            Ok(std::mem::replace(
                &mut iter.stream,
                Box::pin(futures::stream::empty()),
            ))
        })?;
        let value = stream.next().await.map(|v| v.into_cbor().to_vec());
        accessor.with(|mut access| -> wasmtime::Result<()> {
            access.get().0.table().get_mut(&resource)?.stream = stream;
            Ok(())
        })?;
        Ok(value)
    }
}

impl<T: HostView + 'static> HostWithStore<T> for LinkerHost<T> {
    async fn hostcall(
        accessor: &Accessor<T, Self>,
        call_type: String,
        payload: Vec<u8>,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        let host = accessor.with(|mut access| Arc::clone(access.get().0.host()));
        Ok(wasmtime_wasi::runtime::spawn(
            async move {
                if call_type == HTTP_HOSTCALL_TYPE {
                    return http_hostcall(host, payload).await;
                }

                let payload = Value::from_cbor(payload);
                host.hostcall(&call_type, payload)
                    .await
                    .map(|v| v.into_cbor().to_vec())
                    .map_err(|e| e.to_string())
            }
            .in_current_span(),
        )
        .await)
    }
}

async fn http_hostcall<H: crate::host::Host>(
    host: Arc<H>,
    payload: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let request: HttpHostcallRequest = Value::from_cbor(payload)
        .to_serde()
        .map_err(|e| e.to_string())?;

    let mut req = http::Request::new(request.body.map(Into::into));
    *req.method_mut() = request
        .method
        .parse()
        .map_err(|e| format!("invalid HTTP method: {e}"))?;
    *req.uri_mut() = request
        .url
        .parse()
        .map_err(|e| format!("invalid HTTP URL: {e}"))?;
    for (name, value) in request.headers {
        let name = http::HeaderName::from_bytes(name.as_bytes()).map_err(|e| e.to_string())?;
        let value = http::HeaderValue::from_bytes(&value).map_err(|e| e.to_string())?;
        req.headers_mut().append(name, value);
    }

    let deadline = match request.timeout_ms {
        Some(timeout_ms) => Some(
            Instant::now()
                .checked_add(Duration::from_millis(timeout_ms))
                .ok_or_else(|| format!("HTTP timeout is too large: {timeout_ms}ms"))?,
        ),
        None => None,
    };
    let response = with_optional_deadline(deadline, host.http_request(req))
        .await
        .ok_or_else(|| timeout_message(request.timeout_ms))?
        .map_err(|e| e.to_string())?;
    let (parts, body) = response.into_parts();
    let mut body_stream = body;
    let mut body = BytesMut::new();
    while let Some(frame) = with_optional_deadline(deadline, body_stream.next())
        .await
        .ok_or_else(|| timeout_message(request.timeout_ms))?
    {
        if let Ok(data) = frame.map_err(|e| e.to_string())?.into_data() {
            if body.len().saturating_add(data.len()) > MAX_HTTP_HOSTCALL_RESPONSE_BODY_BYTES {
                return Err(format!(
                    "HTTP response body exceeds maximum size of {MAX_HTTP_HOSTCALL_RESPONSE_BODY_BYTES} bytes"
                ));
            }
            body.extend_from_slice(&data);
        }
    }

    let response = HttpHostcallResponse {
        status: parts.status.as_u16(),
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
            .collect(),
        body: body.freeze().to_vec(),
    };
    Value::from_serde(&response)
        .map(|v| v.into_cbor().to_vec())
        .map_err(|e| e.to_string())
}

async fn with_optional_deadline<F: Future>(
    deadline: Option<Instant>,
    future: F,
) -> Option<F::Output> {
    match deadline {
        Some(deadline) => timeout_at(TokioInstant::from_std(deadline), future)
            .await
            .ok(),
        None => Some(future.await),
    }
}

fn timeout_message(timeout_ms: Option<u64>) -> String {
    timeout_ms.map_or_else(
        || "HTTP request timed out".to_string(),
        |timeout_ms| format!("HTTP request timed out after {timeout_ms}ms"),
    )
}
