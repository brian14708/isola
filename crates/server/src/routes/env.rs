use std::str::FromStr;

use axum::async_trait;
use http::{HeaderName, HeaderValue};
use opentelemetry_semantic_conventions::trace;
use tracing::{field::Empty, Instrument};

use promptkit_executor::Env;

#[derive(Clone)]
pub struct VmEnv {
    pub http: reqwest::Client,
}

#[async_trait]
impl Env for VmEnv {
    async fn send_request(&self, mut req: reqwest::Request) -> reqwest::Result<reqwest::Response> {
        let span = tracing::span!(
            target: "http",
            tracing::Level::INFO,
            "http_client::fetch_reqwest",
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

        let resp = match self.http.execute(req).instrument(span.clone()).await {
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
