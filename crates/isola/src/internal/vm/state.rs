use std::{path::Path, sync::Arc};

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tokio::time::timeout;
use tracing::{Instrument, event};
use url::Url;
use wasmtime::{
    Engine, Store,
    component::{Linker, ResourceTable},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    HttpResult, WasiHttpCtx, WasiHttpView,
    bindings::http::outgoing_handler::ErrorCode,
    body::{HyperIncomingBody, HyperOutgoingBody},
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use super::bindgen::{EmitValue, HostView, add_to_linker};
use crate::{
    Host, NetworkPolicy, OutputSink,
    host::{HttpRequest, HttpResponse},
    internal::{resource::MemoryLimiter, trace_output::TraceOutput, wasm},
};

pub struct InstanceState<H: Host + Clone> {
    pub(crate) limiter: MemoryLimiter,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    host: H,
    policy: Arc<dyn NetworkPolicy>,
    max_redirects: usize,

    sink: Option<Box<dyn OutputSink>>,
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

impl<H: Host + Clone> InstanceState<H> {
    /// Creates a new linker for the VM state.
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

    /// Creates a new VM state with the specified configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the preopened directories cannot be added to the WASI context.
    pub fn new(
        engine: &Engine,
        lib_dir: &Path,
        workdir: Option<&Path>,
        max_memory: usize,
        host: H,
        policy: Arc<dyn NetworkPolicy>,
        max_redirects: usize,
    ) -> anyhow::Result<Store<Self>> {
        let mut builder = WasiCtxBuilder::new();
        builder
            .preopened_dir(lib_dir, "/lib", DirPerms::READ, FilePerms::READ)
            .map_err(|e| anyhow::anyhow!("Failed to add lib_dir to WASI context: {e}"))?;

        if let Some(workdir) = workdir {
            builder
                .preopened_dir(workdir, "/workdir", DirPerms::READ, FilePerms::READ)
                .map_err(|e| anyhow::anyhow!("Failed to add workdir to WASI context: {e}"))?;
        }
        let wasi = builder
            .allow_tcp(false)
            .allow_udp(false)
            .stdout(TraceOutput::new("stdout"))
            .stderr(TraceOutput::new("stderr"))
            .build();
        let limiter = MemoryLimiter::new(max_memory);

        let mut s = Store::new(
            engine,
            Self {
                limiter,
                wasi,
                http: WasiHttpCtx::new(),
                table: ResourceTable::new(),
                host,
                policy,
                max_redirects,
                sink: None,
                output_buffer: OutputBuffer::new(),
            },
        );
        s.limiter(|s| &mut s.limiter);
        Ok(s)
    }

    pub(crate) fn set_sink(&mut self, sink: Option<Box<dyn OutputSink>>) {
        // Prevent cross-call output leakage and avoid retaining large buffers if
        // the call traps or is interrupted mid-output.
        self.output_buffer.reset();
        self.sink = sink;
    }
}

impl<H: Host + Clone> WasiView for InstanceState<H> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl<H: Host + Clone> WasiHttpView for InstanceState<H> {
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
        let host = self.host.clone();
        let policy = Arc::clone(&self.policy);
        let max_redirects = self.max_redirects;

        let handle = wasmtime_wasi::runtime::spawn(
            async move {
                let (parts, body) = request.into_parts();
                let mut headers = parts.headers;

                // `isola` owns redirect-following, so `Host` must not be influenced by stale host headers.
                headers.remove(http::header::HOST);

                // Enforce policy before reading the (potentially unbounded) request body.
                let meta = crate::net::HttpMeta {
                    method: parts.method.clone(),
                    uri: parts.uri.clone(),
                };
                if let Err(reason) = policy.check_http(&meta).await {
                    event!(
                        name: "net.deny",
                        target: TRACE_TARGET_SCRIPT,
                        tracing::Level::DEBUG,
                        net.kind = "http",
                        net.reason = &reason,
                        url.full = parts.uri.to_string(),
                    );
                    return Err(ErrorCode::HttpRequestDenied.into());
                }

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

                let req = HttpFollowRequest {
                    method: parts.method,
                    uri: parts.uri,
                    headers,
                    body,
                };
                let follow_cfg = HttpFollowConfig {
                    max_redirects,
                    first_byte_timeout: config.first_byte_timeout,
                };
                let resp = http_with_redirects(&host, &*policy, req, follow_cfg).await?;

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

fn method_rewrite_and_body_drop(
    status: http::StatusCode,
    method: &http::Method,
) -> (http::Method, bool) {
    match status.as_u16() {
        303 => (http::Method::GET, true),
        301 | 302 => {
            if *method == http::Method::GET || *method == http::Method::HEAD {
                (method.clone(), false)
            } else {
                (http::Method::GET, true)
            }
        }
        _ => (method.clone(), false),
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn apply_redirect_header_hygiene(headers: &mut http::HeaderMap, origin_changed: bool) {
    // `Host` is stripped per-hop in the main loop before handing to the embedder,
    // so we only need to remove cross-origin sensitive headers here.
    if origin_changed {
        headers.remove(http::header::AUTHORIZATION);
        headers.remove(http::header::COOKIE);
        headers.remove("x-promptkit-proxy");
    }
}

async fn http_with_redirects(
    host: &impl Host,
    policy: &dyn NetworkPolicy,
    mut req: HttpFollowRequest,
    cfg: HttpFollowConfig,
) -> Result<HttpResponse, ErrorCode> {
    let mut redirects = 0usize;

    loop {
        let meta = crate::net::HttpMeta {
            method: req.method.clone(),
            uri: req.uri.clone(),
        };
        if let Err(reason) = policy.check_http(&meta).await {
            event!(
                name: "net.deny",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::DEBUG,
                net.kind = "http",
                net.reason = &reason,
                url.full = req.uri.to_string(),
            );
            return Err(ErrorCode::HttpRequestDenied);
        }

        let http_req = HttpRequest {
            method: req.method.clone(),
            uri: req.uri.clone(),
            headers: {
                let mut headers = req.headers.clone();
                // Always drop `Host` before handing the request to the embedder.
                headers.remove(http::header::HOST);
                headers
            },
            body: req.body.clone(),
        };

        let resp = timeout(cfg.first_byte_timeout, host.http_request(http_req))
            .await
            .map_err(|_e| ErrorCode::HttpResponseTimeout)?
            .map_err(|e| ErrorCode::InternalError(Some(format!("request error: {e}"))))?;

        if !resp.status().is_redirection() {
            return Ok(resp);
        }

        let status = resp.status();
        let location = resp.headers().get(http::header::LOCATION).cloned();
        let Some(location) = location else {
            // No `Location`, so there's nothing to follow.
            return Ok(resp);
        };

        let Ok(location_str) = location.to_str() else {
            // Invalid `Location`, return the response as-is.
            return Ok(resp);
        };

        let Ok(base_url) = Url::parse(&req.uri.to_string()) else {
            return Ok(resp);
        };

        let Ok(next_url) = Url::parse(location_str).or_else(|_| base_url.join(location_str)) else {
            // Invalid `Location`, return the response as-is.
            return Ok(resp);
        };

        if next_url.scheme() != "http" && next_url.scheme() != "https" {
            // Only http/https redirects are supported.
            return Ok(resp);
        }

        let redirect_uri: http::Uri = match next_url.as_str().parse() {
            Ok(u) => u,
            Err(_) => return Ok(resp),
        };

        if redirects >= cfg.max_redirects {
            return Err(ErrorCode::LoopDetected);
        }
        redirects += 1;

        event!(
            name: "http.redirect",
            target: TRACE_TARGET_SCRIPT,
            tracing::Level::DEBUG,
            http.status = status.as_u16(),
            url.from = base_url.as_str(),
            url.to = next_url.as_str(),
        );

        let (new_method, drop_body) = method_rewrite_and_body_drop(status, &req.method);
        req.method = new_method;
        if drop_body {
            req.body = None;
            req.headers.remove(http::header::CONTENT_LENGTH);
        }

        let origin_changed = !same_origin(&base_url, &next_url);
        apply_redirect_header_hygiene(&mut req.headers, origin_changed);

        req.uri = redirect_uri;
    }
}

struct HttpFollowConfig {
    max_redirects: usize,
    first_byte_timeout: std::time::Duration,
}

struct HttpFollowRequest {
    method: http::Method,
    uri: http::Uri,
    headers: http::HeaderMap,
    body: Option<Bytes>,
}

impl<H: Host + Clone> HostView for InstanceState<H> {
    type Host = H;

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn host(&mut self) -> &mut Self::Host {
        &mut self.host
    }

    fn network_policy(&self) -> &dyn NetworkPolicy {
        &*self.policy
    }

    async fn emit(&mut self, data: EmitValue) -> wasmtime::Result<()> {
        let Some(sink) = self.sink.as_mut() else {
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
                sink.on_end(output)
                    .await
                    .map_err(|e| anyhow::Error::msg(e.to_string()))
            }
            EmitValue::PartialResult(new_data) => {
                self.output_buffer.append(new_data.as_ref())?;
                let output = self.output_buffer.take();
                sink.on_partial(output)
                    .await
                    .map_err(|e| anyhow::Error::msg(e.to_string()))
            }
        }
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
    use super::*;
    use crate::{BoxError, Host};
    use http_body::Frame;
    use http_body_util::BodyExt as _;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use wasmtime_wasi::p2::Pollable as _;
    use wasmtime_wasi_http::bindings::http::types::ErrorCode as TypesErrorCode;

    #[derive(Clone, Default)]
    struct ScriptedHost {
        calls: Arc<Mutex<Vec<crate::HttpRequest>>>,
    }

    impl ScriptedHost {
        fn calls(&self) -> Vec<crate::HttpRequest> {
            self.calls.lock().expect("lock poisoned").clone()
        }
    }

    fn empty_body() -> crate::HttpBodyStream {
        Box::pin(futures::stream::empty::<Result<Frame<Bytes>, BoxError>>())
    }

    #[async_trait::async_trait]
    impl Host for ScriptedHost {
        async fn hostcall(
            &self,
            _call_type: &str,
            _payload: Bytes,
        ) -> core::result::Result<Bytes, BoxError> {
            Err(std::io::Error::other("unsupported").into())
        }

        async fn http_request(
            &self,
            req: crate::HttpRequest,
        ) -> core::result::Result<crate::HttpResponse, BoxError> {
            self.calls.lock().expect("lock poisoned").push(req.clone());

            let uri = req.uri.to_string();
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

        async fn websocket_connect(
            &self,
            _req: crate::WebsocketRequest,
        ) -> core::result::Result<crate::WebsocketResponse, BoxError> {
            Err(std::io::Error::other("unsupported").into())
        }
    }

    struct DenySecondHop;

    #[async_trait::async_trait]
    impl NetworkPolicy for DenySecondHop {
        async fn check_http(
            &self,
            meta: &crate::net::HttpMeta,
        ) -> core::result::Result<(), String> {
            if meta.uri.host() == Some("b.example") {
                return Err("denied".to_string());
            }
            Ok(())
        }

        async fn check_websocket(
            &self,
            _meta: &crate::net::WebsocketMeta,
        ) -> core::result::Result<(), String> {
            Ok(())
        }
    }

    struct DenyAllPolicy;

    #[async_trait::async_trait]
    impl NetworkPolicy for DenyAllPolicy {
        async fn check_http(
            &self,
            _meta: &crate::net::HttpMeta,
        ) -> core::result::Result<(), String> {
            Err("denied".to_string())
        }

        async fn check_websocket(
            &self,
            _meta: &crate::net::WebsocketMeta,
        ) -> core::result::Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn send_request_checks_policy_before_reading_body() {
        let host = ScriptedHost::default();

        let mut state = InstanceState {
            limiter: MemoryLimiter::new(1024),
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            host: host.clone(),
            policy: Arc::new(DenyAllPolicy),
            max_redirects: 10,
            sink: None,
            output_buffer: OutputBuffer::new(),
        };

        // A body that never completes. Before the fix, this would hang forever because the body
        // was read before policy enforcement.
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
            connect_timeout: Duration::from_millis(10),
            first_byte_timeout: Duration::from_secs(1),
            between_bytes_timeout: Duration::from_secs(1),
        };

        let mut fut = state.send_request(req, cfg).expect("send_request");
        timeout(Duration::from_millis(200), fut.ready())
            .await
            .expect("ready in time");

        let err = match fut.unwrap_ready() {
            Ok(Ok(_)) => panic!("expected deny"),
            Ok(Err(e)) => e,
            Err(e) => e.downcast::<ErrorCode>().expect("downcast ErrorCode"),
        };
        assert!(matches!(err, ErrorCode::HttpRequestDenied));
        assert!(host.calls().is_empty());
    }

    #[tokio::test]
    async fn send_request_body_timeout_is_enforced() {
        let host = ScriptedHost::default();

        let mut state = InstanceState {
            limiter: MemoryLimiter::new(1024),
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            host: host.clone(),
            policy: Arc::new(crate::net::AllowAllPolicy),
            max_redirects: 10,
            sink: None,
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
    async fn follows_redirect_and_sanitizes_headers() {
        let host = ScriptedHost::default();
        let policy = crate::net::AllowAllPolicy;
        let mut headers = http::HeaderMap::new();

        headers.insert(http::header::HOST, "a.example".parse().expect("header"));
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer secret".parse().expect("header"),
        );
        headers.insert(http::header::COOKIE, "a=b".parse().expect("header"));
        headers.insert(
            http::HeaderName::from_static("x-promptkit-proxy"),
            "http://proxy".parse().expect("header"),
        );
        headers.insert(
            http::HeaderName::from_static("x-other"),
            "keep".parse().expect("header"),
        );

        let req = HttpFollowRequest {
            method: http::Method::POST,
            uri: "http://a.example/".parse().expect("uri"),
            headers,
            body: Some(Bytes::from_static(b"body")),
        };

        let cfg = HttpFollowConfig {
            max_redirects: 10,
            first_byte_timeout: Duration::from_secs(1),
        };

        let resp = http_with_redirects(&host, &policy, req, cfg)
            .await
            .expect("redirect follow");
        assert_eq!(resp.status(), http::StatusCode::OK);

        let calls = host.calls();
        assert_eq!(calls.len(), 2);

        assert_eq!(calls[0].method, http::Method::POST);
        assert!(calls[0].headers.get(http::header::HOST).is_none());
        assert_eq!(calls[0].body.as_deref(), Some(&b"body"[..]));

        assert_eq!(calls[1].method, http::Method::GET);
        assert!(calls[1].headers.get(http::header::AUTHORIZATION).is_none());
        assert!(calls[1].headers.get(http::header::COOKIE).is_none());
        assert!(calls[1].headers.get("x-promptkit-proxy").is_none());
        assert_eq!(
            calls[1]
                .headers
                .get("x-other")
                .expect("x-other preserved")
                .to_str()
                .expect("valid header value"),
            "keep"
        );
        assert!(calls[1].body.is_none());
    }

    #[tokio::test]
    async fn policy_applies_to_every_hop() {
        let host = ScriptedHost::default();
        let policy = DenySecondHop;

        let req = HttpFollowRequest {
            method: http::Method::GET,
            uri: "http://a.example/".parse().expect("uri"),
            headers: http::HeaderMap::new(),
            body: None,
        };

        let cfg = HttpFollowConfig {
            max_redirects: 10,
            first_byte_timeout: Duration::from_secs(1),
        };

        let err = match http_with_redirects(&host, &policy, req, cfg).await {
            Ok(_) => panic!("expected deny"),
            Err(e) => e,
        };
        assert!(matches!(err, ErrorCode::HttpRequestDenied));
        assert_eq!(host.calls().len(), 1);
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

    #[test]
    fn method_rewrite_behavior() {
        let (m, drop) =
            method_rewrite_and_body_drop(http::StatusCode::SEE_OTHER, &http::Method::POST);
        assert_eq!(m, http::Method::GET);
        assert!(drop);

        let (m, drop) = method_rewrite_and_body_drop(http::StatusCode::FOUND, &http::Method::GET);
        assert_eq!(m, http::Method::GET);
        assert!(!drop);

        let (m, drop) =
            method_rewrite_and_body_drop(http::StatusCode::TEMPORARY_REDIRECT, &http::Method::POST);
        assert_eq!(m, http::Method::POST);
        assert!(!drop);
    }
}
