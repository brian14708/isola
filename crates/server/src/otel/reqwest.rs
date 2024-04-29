use std::str::FromStr;

use axum::async_trait;
use http::{Extensions, HeaderName, HeaderValue};
use opentelemetry_semantic_conventions::trace;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result};
use tracing::{field::Empty, Instrument};

pub struct OtelMiddleware();

#[async_trait]
impl Middleware for OtelMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let span = tracing::span!(
            tracing::Level::INFO,
            "http_client::fetch_reqwest",
            promptkit.kind = "http",
            promptkit.user = true,
            otel.kind = "client",
            { trace::HTTP_REQUEST_METHOD } = req.method().as_str(),
            { trace::SERVER_ADDRESS } = req.url().host_str().unwrap_or_default(),
            { trace::SERVER_PORT } = req.url().port_or_known_default().unwrap_or_default(),
            { trace::URL_FULL } = req.url().to_string(),
            { trace::HTTP_RESPONSE_STATUS_CODE } = Empty,
            { trace::OTEL_STATUS_CODE } = Empty,
        );
        opentelemetry::global::get_text_map_propagator(|injector| {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            struct RequestCarrier<'a> {
                request: &'a mut reqwest::Request,
            }
            impl<'a> opentelemetry::propagation::Injector for RequestCarrier<'a> {
                fn set(&mut self, key: &str, value: String) {
                    let header_name = HeaderName::from_str(key).expect("Must be header name");
                    let header_value =
                        HeaderValue::from_str(&value).expect("Must be a header value");
                    self.request.headers_mut().insert(header_name, header_value);
                }
            }

            let context = span.context();
            injector.inject_context(&context, &mut RequestCarrier { request: &mut req });
        });

        let resp = match next.run(req, extensions).instrument(span.clone()).await {
            Ok(resp) => resp,
            Err(err) => {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
                return Err(err);
            }
        };

        let status = resp.status();
        span.record("http.response.status_code", status.as_u16());
        if status.is_server_error() || status.is_client_error() {
            span.record(trace::OTEL_STATUS_CODE, "ERROR");
        } else {
            span.record(trace::OTEL_STATUS_CODE, "OK");
        }

        Ok(resp)
    }
}
