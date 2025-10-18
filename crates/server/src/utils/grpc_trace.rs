use std::time::Duration;

use http::{Response, Uri};
use opentelemetry_semantic_conventions::attribute as trace;
use promptkit_trace::consts::TRACE_TARGET_OTEL;
use tower_http::{
    classify::{GrpcErrorsAsFailures, GrpcFailureClass, SharedClassifier},
    trace::{OnFailure, OnResponse},
};
use tracing::{Span, field::Empty};

pub fn grpc_server_tracing_layer() -> tower_http::trace::TraceLayer<
    SharedClassifier<GrpcErrorsAsFailures>,
    MakeSpan,
    (),
    OtelOnResponse,
    (),
    (),
    OtelOnGrpcFailure,
> {
    tower_http::trace::TraceLayer::new_for_grpc()
        .make_span_with(MakeSpan)
        .on_response(OtelOnResponse)
        .on_failure(OtelOnGrpcFailure)
        .on_eos(())
        .on_request(())
        .on_body_chunk(())
}

#[derive(Clone)]
pub struct MakeSpan;

impl<B> tower_http::trace::MakeSpan<B> for MakeSpan {
    fn make_span(&mut self, request: &http::Request<B>) -> tracing::Span {
        struct HeaderExtractor<'a>(&'a http::HeaderMap);
        impl opentelemetry::propagation::Extractor for HeaderExtractor<'_> {
            fn get(&self, key: &str) -> Option<&str> {
                self.0.get(key).and_then(|value| value.to_str().ok())
            }

            fn keys(&self) -> Vec<&str> {
                self.0.keys().map(http::HeaderName::as_str).collect()
            }
        }
        let extractor = HeaderExtractor(request.headers());
        let remote_context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&extractor)
        });

        let (service, method) = extract_service_method(request.uri());
        let server_addr = request
            .headers()
            .get(http::header::HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| request.uri().host())
            .unwrap_or("");
        let span = tracing::info_span!(
            target: TRACE_TARGET_OTEL,
            "grpc.server",
            { trace::RPC_SYSTEM } = "grpc",
            { trace::RPC_SERVICE } = service,
            { trace::RPC_METHOD } = method,
            { trace::SERVER_ADDRESS } = server_addr,
            { trace::SERVER_PORT } = request.uri().port_u16(),
            { trace::RPC_GRPC_STATUS_CODE } = Empty,
            { trace::OTEL_STATUS_CODE } = Empty,
            { trace::OTEL_STATUS_DESCRIPTION } = Empty,
            otel.kind = "server",
        );
        _ = tracing_opentelemetry::OpenTelemetrySpanExt::set_parent(&span, remote_context);
        span
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
        span.record(trace::OTEL_STATUS_CODE, "OK");
    }
}

#[derive(Clone)]
pub struct OtelOnGrpcFailure;

impl OnFailure<GrpcFailureClass> for OtelOnGrpcFailure {
    fn on_failure(&mut self, failure: GrpcFailureClass, _latency: Duration, span: &Span) {
        span.record(trace::OTEL_STATUS_CODE, "ERROR");
        match failure {
            GrpcFailureClass::Code(code) => {
                span.record(trace::RPC_GRPC_STATUS_CODE, code);
            }
            GrpcFailureClass::Error(msg) => {
                span.record(trace::RPC_GRPC_STATUS_CODE, 2);
                span.record(trace::OTEL_STATUS_DESCRIPTION, msg);
            }
        }
    }
}
