use std::str::FromStr;

use bytes::Bytes;
use futures::Stream;
use http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tracing::Span;

use crate::{
    Error, RequestOptions, WebsocketMessage, options::RequestContext, trace::TraceRequest,
};

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

const USER_AGENT: &str = "PromptKit/1.0";

impl Client {
    pub fn new() -> Self {
        let cli = reqwest::Client::builder().user_agent(USER_AGENT);
        Self {
            http: cli.build().unwrap(),
        }
    }

    pub fn http<B, C>(
        &self,
        request: http::Request<B>,
        mut options: RequestOptions<C>,
    ) -> impl Future<
        Output = Result<
            http::Response<
                impl Stream<Item = Result<http_body::Frame<Bytes>, Error>> + Send + 'static,
            >,
            Error,
        >,
    > + Send
    + 'static
    where
        B: http_body::Body + Send + Sync + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
        C: RequestContext,
    {
        let (parts, body) = request.into_parts();
        let span = options.context.make_span(&TraceRequest::Http(&parts));
        let mut request = http::Request::from_parts(parts, body);
        inject_headers(&span, &mut request);
        let http = if let Some(p) = request.headers_mut().remove("x-promptkit-proxy") {
            reqwest::Proxy::all(p.to_str().unwrap_or_default())
                .and_then(|p| {
                    reqwest::Client::builder()
                        .user_agent(USER_AGENT)
                        .proxy(p)
                        .build()
                })
                .map_err(|e| e.into())
        } else {
            Ok(self.http.clone())
        };
        crate::http::http_impl(span, http, request)
    }

    pub fn websocket<C: RequestContext>(
        &self,
        request: http::Request<impl Stream<Item = WebsocketMessage> + Send + 'static>,
        mut options: RequestOptions<C>,
    ) -> impl Future<
        Output = Result<
            http::Response<impl Stream<Item = Result<WebsocketMessage, Error>> + Send + 'static>,
            Error,
        >,
    > + Send
    + 'static {
        let (parts, body) = request.into_parts();
        let span = options.context.make_span(&TraceRequest::Http(&parts));
        let mut request = http::Request::from_parts(parts, body);
        inject_headers(&span, &mut request);
        let headers = request.headers_mut();
        headers.insert(
            HeaderName::from_static("connection"),
            HeaderValue::from_static("Upgrade"),
        );
        headers.insert(
            HeaderName::from_static("upgrade"),
            HeaderValue::from_static("websocket"),
        );
        headers.insert(
            HeaderName::from_static("sec-websocket-version"),
            HeaderValue::from_static("13"),
        );
        headers.insert(
            HeaderName::from_static("sec-websocket-key"),
            HeaderValue::try_from(generate_key()).unwrap(),
        );
        crate::http::websocket_impl(span, self.http.clone(), request)
    }
}

fn inject_headers<B>(span: &Span, request: &mut http::Request<B>) {
    opentelemetry::global::get_text_map_propagator(|injector| {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        struct RequestCarrier<'a, T>(&'a mut http::Request<T>);
        impl<T> opentelemetry::propagation::Injector for RequestCarrier<'_, T> {
            fn set(&mut self, key: &str, value: String) {
                let header_name = HeaderName::from_str(key).expect("Must be header name");
                let header_value = HeaderValue::try_from(value).expect("Must be a header value");
                self.0.headers_mut().insert(header_name, header_value);
            }
        }

        let context = span.context();
        injector.inject_context(&context, &mut RequestCarrier(request));
    });
}
