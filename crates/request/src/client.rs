use std::str::FromStr;

use bytes::Bytes;
use futures::Stream;
use http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tracing::Span;

use crate::{Error, RequestOptions, grpc::GrpcPool, options::RequestContext};
use crate::{WebsocketMessage, trace::TraceRequest};

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    grpc: GrpcPool,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let cli = reqwest::Client::builder().user_agent("PromptKit/1.0");
        Self {
            http: cli.build().unwrap(),
            grpc: GrpcPool::new(),
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
        crate::http::http_impl(span, self.http.clone(), request)
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

    pub fn grpc<C: RequestContext>(
        &self,
        request: http::Request<impl Stream<Item = Bytes> + Send + 'static>,
        mut options: RequestOptions<C>,
    ) -> impl Future<
        Output = Result<
            http::Response<impl Stream<Item = Result<Bytes, Error>> + Send + 'static>,
            Error,
        >,
    > + Send
    + 'static {
        let (parts, body) = request.into_parts();
        let span = options.context.make_span(&TraceRequest::Grpc(&parts));
        let mut request = http::Request::from_parts(parts, body);
        inject_headers(&span, &mut request);
        crate::grpc::grpc(span, self.grpc.clone(), request)
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
