use axum::{
    body::Body,
    http::{HeaderMap, Request},
};
use opentelemetry::{Context, global, propagation::Extractor};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub const SCRIPT_TRACE_TARGET: &str = "isola_server::script";

struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}

pub fn extract_parent_context(headers: &HeaderMap) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

pub fn attach_parent_context(span: &Span, parent_context: Context) {
    if let Err(err) = span.set_parent(parent_context) {
        tracing::debug!(?err, "failed to attach parent trace context");
    }
}

pub fn make_server_span(request: &Request<Body>) -> Span {
    let span = tracing::span!(
        target: SCRIPT_TRACE_TARGET,
        tracing::Level::INFO,
        "http.server",
        http.method = %request.method(),
        http.route = %request.uri().path(),
    );

    let parent_context = extract_parent_context(request.headers());
    attach_parent_context(&span, parent_context);
    span
}
