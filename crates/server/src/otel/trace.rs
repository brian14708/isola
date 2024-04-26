use std::time::Duration;

use http_02::{Response, Uri};
use opentelemetry::trace::{TraceContextExt, TraceId};
use opentelemetry_sdk::trace::RandomIdGenerator;
use tower_http_04::{
    classify::{GrpcErrorsAsFailures, GrpcFailureClass, SharedClassifier},
    trace::{DefaultOnBodyChunk, DefaultOnEos, DefaultOnRequest, OnFailure, OnResponse},
};
use tracing::{field::Empty, Span};

pub fn grpc_server_tracing_layer() -> tower_http_04::trace::TraceLayer<
    SharedClassifier<GrpcErrorsAsFailures>,
    MakeSpan,
    DefaultOnRequest,
    OtelOnResponse,
    DefaultOnBodyChunk,
    DefaultOnEos,
    OtelOnGrpcFailure,
> {
    tower_http_04::trace::TraceLayer::new_for_grpc()
        .make_span_with(MakeSpan)
        .on_response(OtelOnResponse)
        .on_failure(OtelOnGrpcFailure)
}

#[derive(Clone)]
pub struct MakeSpan;

impl<B> tower_http_04::trace::MakeSpan<B> for MakeSpan {
    fn make_span(&mut self, request: &http_02::Request<B>) -> tracing::Span {
        struct HeaderExtractor<'a>(&'a http_02::HeaderMap);
        impl<'a> opentelemetry::propagation::Extractor for HeaderExtractor<'a> {
            fn get(&self, key: &str) -> Option<&str> {
                self.0.get(key).and_then(|value| value.to_str().ok())
            }

            fn keys(&self) -> Vec<&str> {
                self.0.keys().map(http_02::HeaderName::as_str).collect()
            }
        }
        let extractor = HeaderExtractor(request.headers());
        let (remote_context, _) =
            create_context_with_trace(opentelemetry::global::get_text_map_propagator(
                |propagator| propagator.extract(&extractor),
            ));

        let (service, method) = extract_service_method(request.uri());

        let server_addr = request
            .headers()
            .get(http_02::header::HOST)
            .map_or(request.uri().host(), |h| h.to_str().ok())
            .unwrap_or("");
        let span = tracing::info_span!(
            "gRPC request",
            rpc.system = "grpc",
            rpc.service = service,
            rpc.method = method,
            server.address = server_addr,
            server.port = request.uri().port_u16().unwrap_or(0),
            rpc.grpc.status_code = Empty,
            otel.status_code = Empty,
            otel.status_message = Empty,
            otel.kind = "server",
            promptkit.user = true,
        );
        tracing_opentelemetry::OpenTelemetrySpanExt::set_parent(&span, remote_context);
        span
    }
}

fn create_context_with_trace(
    remote_context: opentelemetry::Context,
) -> (opentelemetry::Context, TraceId) {
    if remote_context.span().span_context().is_valid() {
        let remote_span = remote_context.span();
        let span_context = remote_span.span_context();
        let trace_id = span_context.trace_id();
        (remote_context, trace_id)
    } else {
        // create a fake remote context but with a fresh new trace_id
        use opentelemetry::trace::{SpanContext, SpanId};
        use opentelemetry_sdk::trace::IdGenerator;
        let trace_id = RandomIdGenerator::default().new_trace_id();
        let new_span_context = SpanContext::new(
            trace_id,
            SpanId::INVALID,
            remote_context.span().span_context().trace_flags(),
            false,
            remote_context.span().span_context().trace_state().clone(),
        );
        (
            remote_context.with_remote_span_context(new_span_context),
            trace_id,
        )
    }
}

pub fn extract_service_method(uri: &Uri) -> (&str, &str) {
    let path = uri.path();
    let mut parts = path.split('/').filter(|x| !x.is_empty());
    let service = parts.next().unwrap_or_default();
    let method = parts.next().unwrap_or_default();
    (service, method)
}

#[derive(Clone)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, _response: &Response<B>, _latency: Duration, span: &Span) {
        span.record("otel.status_code", "ok");
    }
}

#[derive(Clone)]
pub struct OtelOnGrpcFailure;

impl OnFailure<GrpcFailureClass> for OtelOnGrpcFailure {
    fn on_failure(&mut self, failure: GrpcFailureClass, _latency: Duration, span: &Span) {
        span.record("otel.status_code", "error");
        match failure {
            GrpcFailureClass::Code(code) => {
                span.record("rpc.grpc.status_code", code);
            }
            GrpcFailureClass::Error(msg) => {
                span.record("rpc.grpc.status_code", 2);
                span.record("otel.status_message", msg);
            }
        }
    }
}
