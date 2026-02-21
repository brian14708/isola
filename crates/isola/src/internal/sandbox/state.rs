use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use tokio::time::timeout;
use tracing::Instrument;
use wasmtime::{
    Engine, Store,
    component::{Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    HttpResult, WasiHttpCtx, WasiHttpView,
    bindings::http::outgoing_handler::ErrorCode,
    body::{HyperIncomingBody, HyperOutgoingBody},
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use super::bindings::{EmitValue, HostView, add_to_linker};
use crate::{
    host::{Host, HttpRequest, LogContext, LogLevel, OutputSink},
    internal::{
        resource::MemoryLimiter,
        trace_output::{LogSinkStore, TraceOutput, new_log_sink_store, set_log_sink},
        wasm,
    },
    sandbox::DirectoryMapping,
    value::Value,
};

pub struct InstanceState<H: Host> {
    pub limiter: MemoryLimiter,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    host: Arc<H>,

    sink: Option<Arc<dyn OutputSink>>,
    log_sink_store: LogSinkStore,
    output_buffer: OutputBuffer,
}

const MAX_OUTGOING_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTGOING_HTTP_BODY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_BUFFERED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

async fn collect_outgoing_http_body(
    mut body: HyperOutgoingBody,
    max_bytes: usize,
    read_timeout: std::time::Duration,
) -> Result<Option<Bytes>, ErrorCode> {
    let bytes = timeout(read_timeout, async {
        let mut buf = BytesMut::new();
        while let Some(frame) = http_body_util::BodyExt::frame(&mut body).await {
            let frame = frame.map_err(|e| {
                ErrorCode::InternalError(Some(format!("request body read error: {e:?}")))
            })?;

            if let Ok(data) = frame.into_data() {
                if buf.len().saturating_add(data.len()) > max_bytes {
                    return Err(ErrorCode::HttpRequestBodySize(Some(
                        u64::try_from(max_bytes).unwrap_or(u64::MAX),
                    )));
                }
                buf.extend_from_slice(data.as_ref());
            }
        }

        Ok::<Bytes, ErrorCode>(buf.freeze())
    })
    .await
    .map_err(|_e| ErrorCode::ConnectionWriteTimeout)??;

    Ok(if bytes.is_empty() { None } else { Some(bytes) })
}

impl<H: Host> InstanceState<H> {
    /// Creates a new linker for the sandbox state.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the WASI components fail to link.
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        wasm::logging::add_to_linker(&mut linker)?;
        add_to_linker(&mut linker)?;
        Ok(linker)
    }

    /// Creates a new sandbox state with the specified configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the preopened directories cannot be added to the
    /// WASI context.
    pub fn new(
        engine: &Engine,
        directory_mappings: &[DirectoryMapping],
        env: &[(String, String)],
        max_memory: usize,
        host: H,
    ) -> anyhow::Result<Store<Self>> {
        let log_sink_store = new_log_sink_store();
        let mut builder = WasiCtxBuilder::new();

        for mapping in directory_mappings {
            builder
                .preopened_dir(
                    &mapping.host,
                    &mapping.guest,
                    mapping.dir_perms,
                    mapping.file_perms,
                )
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to add directory mapping '{}' -> '{}': {e}",
                        mapping.host.display(),
                        mapping.guest
                    )
                })?;
        }
        for (k, v) in env {
            builder.env(k, v);
        }
        let wasi = builder
            .allow_tcp(false)
            .allow_udp(false)
            .stdout(TraceOutput::new(
                LogLevel::Stdout,
                LogContext::Stdout,
                Arc::clone(&log_sink_store),
            ))
            .stderr(TraceOutput::new(
                LogLevel::Stderr,
                LogContext::Stderr,
                Arc::clone(&log_sink_store),
            ))
            .build();
        let limiter = MemoryLimiter::new(max_memory);

        let mut s = Store::new(
            engine,
            Self {
                limiter,
                wasi,
                http: WasiHttpCtx::new(),
                table: ResourceTable::new(),
                host: Arc::new(host),
                sink: None,
                log_sink_store,
                output_buffer: OutputBuffer::new(),
            },
        );
        s.limiter(|s| &mut s.limiter);
        Ok(s)
    }

    pub fn set_sink(&mut self, sink: Option<Arc<dyn OutputSink>>) {
        // Prevent cross-call output leakage and avoid retaining large buffers if
        // the call traps or is interrupted mid-output.
        self.output_buffer.reset();
        set_log_sink(&self.log_sink_store, sink.clone());
        self.sink = sink;
    }

    #[allow(clippy::needless_pass_by_ref_mut, clippy::unused_async)]
    pub async fn flush_logs(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<H: Host> WasiView for InstanceState<H> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl<H: Host> WasiHttpView for InstanceState<H> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let host = Arc::clone(&self.host);

        let handle = wasmtime_wasi::runtime::spawn(
            async move {
                let (parts, body) = request.into_parts();
                let headers = parts.headers;

                // Fast-path reject based on `Content-Length` if present.
                if let Some(len) = headers.get(http::header::CONTENT_LENGTH)
                    && let Some(len) = len.to_str().ok().and_then(|s| s.parse::<u64>().ok())
                {
                    let max = u64::try_from(MAX_OUTGOING_HTTP_BODY_BYTES).unwrap_or(u64::MAX);
                    if len > max {
                        return Err(ErrorCode::HttpRequestBodySize(Some(max)).into());
                    }
                }

                let body_timeout = config
                    .connect_timeout
                    .min(MAX_OUTGOING_HTTP_BODY_READ_TIMEOUT);
                let body =
                    collect_outgoing_http_body(body, MAX_OUTGOING_HTTP_BODY_BYTES, body_timeout)
                        .await?;

                let mut req = HttpRequest::new(body);
                *req.method_mut() = parts.method;
                *req.uri_mut() = parts.uri;
                *req.headers_mut() = headers;
                let resp = timeout(config.first_byte_timeout, host.http_request(req))
                    .await
                    .map_err(|_e| ErrorCode::HttpResponseTimeout)?
                    .map_err(|e| ErrorCode::InternalError(Some(format!("request error: {e}"))))?;

                let (part, body) = resp
                    .map(|b| {
                        http_body_util::StreamBody::new(
                            b.map(|e| e.map_err(|e| ErrorCode::InternalError(Some(e.to_string())))),
                        )
                    })
                    .into_parts();

                Ok(Ok(IncomingResponse {
                    resp: hyper::Response::<HyperIncomingBody>::from_parts(
                        part,
                        HyperIncomingBody::new(body),
                    ),
                    worker: None,
                    between_bytes_timeout: config.between_bytes_timeout,
                }))
            }
            .in_current_span(),
        );
        Ok(HostFutureIncomingResponse::pending(handle))
    }
}

impl<H: Host> HostView for InstanceState<H> {
    type Host = H;

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn host(&mut self) -> &Arc<Self::Host> {
        &self.host
    }

    async fn emit(&mut self, data: EmitValue) -> wasmtime::Result<()> {
        let Some(sink) = self.sink.as_ref() else {
            return Err(anyhow::anyhow!("output sink missing"));
        };

        match data {
            EmitValue::Continuation(new_data) => {
                self.output_buffer.append(new_data.as_ref())?;
                Ok(())
            }
            EmitValue::End(new_data) => {
                self.output_buffer.append(new_data.as_ref())?;
                let output = self.output_buffer.take();
                let output = if output.is_empty() {
                    None
                } else {
                    Some(Value::from(output))
                };
                sink.on_complete(output)
                    .await
                    .map_err(anyhow::Error::from_boxed)
            }
            EmitValue::PartialResult(new_data) => {
                self.output_buffer.append(new_data.as_ref())?;
                let output = self.output_buffer.take();
                sink.on_item(Value::from(output))
                    .await
                    .map_err(anyhow::Error::from_boxed)
            }
        }
    }
}

impl<H: Host> wasm::logging::HostView for InstanceState<H> {
    async fn emit_log(
        &mut self,
        log_level: wasm::logging::bindings::logging::Level,
        context: &str,
        message: &str,
    ) -> wasmtime::Result<()> {
        let sink_level = match log_level {
            wasm::logging::bindings::logging::Level::Trace => LogLevel::Trace,
            wasm::logging::bindings::logging::Level::Debug => LogLevel::Debug,
            wasm::logging::bindings::logging::Level::Info => LogLevel::Info,
            wasm::logging::bindings::logging::Level::Warn => LogLevel::Warn,
            wasm::logging::bindings::logging::Level::Error => LogLevel::Error,
            wasm::logging::bindings::logging::Level::Critical => LogLevel::Critical,
        };
        let sink_context = match context {
            "stdout" => LogContext::Stdout,
            "stderr" => LogContext::Stderr,
            _ => LogContext::Other(context),
        };
        let sink_level = match sink_context {
            LogContext::Stdout => LogLevel::Stdout,
            LogContext::Stderr => LogLevel::Stderr,
            LogContext::Other(_) => sink_level,
        };
        if let Some(sink) = self.sink.clone() {
            sink.on_log(sink_level, sink_context, message)
                .await
                .map_err(anyhow::Error::from_boxed)?;
        }
        Ok(())
    }
}

struct OutputBuffer(BytesMut);

impl OutputBuffer {
    fn new() -> Self {
        Self(BytesMut::new())
    }

    #[inline]
    fn reset(&mut self) {
        let _old = std::mem::take(&mut self.0);
    }

    #[inline]
    fn append(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let new_len = self.0.len().saturating_add(data.len());
        if new_len > MAX_BUFFERED_OUTPUT_BYTES {
            // Drop any already-buffered data to avoid retaining attacker-controlled memory.
            self.reset();
            anyhow::bail!("output buffer exceeded hard limit ({MAX_BUFFERED_OUTPUT_BYTES} bytes)");
        }
        self.0.extend_from_slice(data);
        Ok(())
    }

    #[inline]
    fn take(&mut self) -> Bytes {
        std::mem::take(&mut self.0).freeze()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use http_body::Frame;
    use http_body_util::BodyExt as _;
    use parking_lot::Mutex;
    use wasmtime_wasi::p2::Pollable as _;
    use wasmtime_wasi_http::bindings::http::types::ErrorCode as TypesErrorCode;

    use super::*;
    use crate::host::{BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse};

    #[derive(Clone, Default)]
    struct ScriptedHost {
        calls: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl ScriptedHost {
        fn calls(&self) -> Vec<HttpRequest> {
            self.calls.lock().clone()
        }
    }

    fn empty_body() -> HttpBodyStream {
        Box::pin(futures::stream::empty::<Result<Frame<Bytes>, BoxError>>())
    }

    #[async_trait::async_trait]
    impl Host for ScriptedHost {
        async fn hostcall(
            &self,
            _call_type: &str,
            _payload: Value,
        ) -> core::result::Result<Value, BoxError> {
            Err(std::io::Error::other("unsupported").into())
        }

        async fn http_request(
            &self,
            req: HttpRequest,
        ) -> core::result::Result<HttpResponse, BoxError> {
            self.calls.lock().push(req.clone());

            let uri = req.uri().to_string();
            let resp = match uri.as_str() {
                "http://a.example/" => http::Response::builder()
                    .status(http::StatusCode::FOUND)
                    .header(http::header::LOCATION, "http://b.example/next")
                    .body(empty_body())
                    .expect("response build"),
                "http://b.example/next" => http::Response::builder()
                    .status(http::StatusCode::OK)
                    .body(empty_body())
                    .expect("response build"),
                _ => {
                    return Err(std::io::Error::other(format!("unexpected uri: {uri}")).into());
                }
            };
            Ok(resp)
        }
    }

    #[tokio::test]
    async fn send_request_body_timeout_is_enforced() {
        let host = ScriptedHost::default();

        let mut state = InstanceState {
            limiter: MemoryLimiter::new(1024),
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            host: Arc::new(host.clone()),
            sink: None,
            log_sink_store: Arc::new(Mutex::new(None)),
            output_buffer: OutputBuffer::new(),
        };

        // A body that never completes.
        let body: HyperOutgoingBody = http_body_util::StreamBody::new(futures::stream::pending::<
            Result<Frame<Bytes>, TypesErrorCode>,
        >())
        .boxed_unsync();

        let req = hyper::Request::builder()
            .method(http::Method::POST)
            .uri("http://a.example/")
            .body(body)
            .expect("request build");

        let cfg = OutgoingRequestConfig {
            use_tls: false,
            connect_timeout: Duration::from_millis(20),
            first_byte_timeout: Duration::from_secs(1),
            between_bytes_timeout: Duration::from_secs(1),
        };

        let mut fut = state.send_request(req, cfg).expect("send_request");
        timeout(Duration::from_millis(500), fut.ready())
            .await
            .expect("ready in time");

        let err = match fut.unwrap_ready() {
            Ok(Ok(_)) => panic!("expected timeout"),
            Ok(Err(e)) => e,
            Err(e) => e.downcast::<ErrorCode>().expect("downcast ErrorCode"),
        };
        assert!(matches!(err, ErrorCode::ConnectionWriteTimeout));
        assert!(host.calls().is_empty());
    }

    #[tokio::test]
    async fn outgoing_http_body_is_capped() {
        let body: HyperOutgoingBody = http_body_util::StreamBody::new(futures::stream::iter([
            Ok::<_, TypesErrorCode>(Frame::data(Bytes::from_static(b"abcd"))),
            Ok::<_, TypesErrorCode>(Frame::data(Bytes::from_static(b"e"))),
        ]))
        .boxed_unsync();

        let err = collect_outgoing_http_body(body, 4, Duration::from_secs(1))
            .await
            .expect_err("expected cap error");
        assert!(matches!(err, ErrorCode::HttpRequestBodySize(Some(4))));
    }

    #[tokio::test]
    async fn send_request_delegates_redirect_and_host_handling_to_host() {
        let host = ScriptedHost::default();

        let mut state = InstanceState {
            limiter: MemoryLimiter::new(1024),
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            host: Arc::new(host.clone()),
            sink: None,
            log_sink_store: Arc::new(Mutex::new(None)),
            output_buffer: OutputBuffer::new(),
        };

        let body: HyperOutgoingBody = http_body_util::StreamBody::new(futures::stream::empty::<
            Result<Frame<Bytes>, TypesErrorCode>,
        >())
        .boxed_unsync();

        let req = hyper::Request::builder()
            .method(http::Method::POST)
            .uri("http://a.example/")
            .header(http::header::HOST, "a.example")
            .header(http::header::AUTHORIZATION, "Bearer secret")
            .header(http::header::COOKIE, "a=b")
            .header("x-isola-proxy", "http://proxy")
            .header("x-other", "keep")
            .body(body)
            .expect("request build");

        let cfg = OutgoingRequestConfig {
            use_tls: false,
            connect_timeout: Duration::from_secs(1),
            first_byte_timeout: Duration::from_secs(1),
            between_bytes_timeout: Duration::from_secs(1),
        };

        let mut fut = state.send_request(req, cfg).expect("send_request");
        timeout(Duration::from_millis(500), fut.ready())
            .await
            .expect("ready in time");

        let incoming = match fut.unwrap_ready() {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => panic!("expected response, got outgoing handler error: {e:?}"),
            Err(e) => panic!("expected response, got transport error: {e:?}"),
        };
        assert_eq!(incoming.resp.status(), http::StatusCode::FOUND);

        let calls = host.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method(), http::Method::POST);
        assert_eq!(calls[0].uri(), "http://a.example/");
        assert_eq!(calls[0].body().as_deref(), None);

        assert_eq!(
            calls[0]
                .headers()
                .get("x-other")
                .expect("x-other forwarded")
                .to_str()
                .expect("valid header value"),
            "keep"
        );
        assert_eq!(
            calls[0]
                .headers()
                .get(http::header::HOST)
                .expect("host forwarded")
                .to_str()
                .expect("valid header value"),
            "a.example"
        );
    }

    #[test]
    fn output_buffer_take_resets() {
        let mut buf = OutputBuffer::new();
        buf.append(b"hello").expect("append within limit");
        assert_eq!(&buf.take()[..], b"hello");
        assert!(buf.take().is_empty());
    }

    #[test]
    fn output_buffer_hard_cap_resets_buffer() {
        let mut buf = OutputBuffer::new();
        let at_limit = vec![0_u8; MAX_BUFFERED_OUTPUT_BYTES];
        buf.append(&at_limit).expect("append at hard limit");
        assert!(buf.append(b"x").is_err());
        assert!(buf.take().is_empty());
    }
}
