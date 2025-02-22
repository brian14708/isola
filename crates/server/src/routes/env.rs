use std::{
    borrow::Cow,
    future::Future,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures_core::Stream;
use futures_util::StreamExt;
use http::{HeaderName, HeaderValue};
use http_body_util::BodyExt;
use opentelemetry_semantic_conventions::attribute as trace;
use pin_project::pin_project;
use tracing::{Instrument, field::Empty};

use promptkit_executor::Env;

#[derive(Clone)]
pub struct VmEnv {
    pub http: reqwest_middleware::ClientWithMiddleware,
}

impl VmEnv {
    pub fn update(&self) -> Cow<'_, Self> {
        Cow::Borrowed(self)
    }

    fn send_request(
        http: reqwest_middleware::ClientWithMiddleware,
        mut req: reqwest::Request,
    ) -> impl std::future::Future<Output = reqwest_middleware::Result<reqwest::Response>> + Send + 'static
    {
        let span = tracing::span!(
            target: "promptkit::http",
            tracing::Level::INFO,
            "http::request_send",
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
            impl opentelemetry::propagation::Injector for RequestCarrier<'_> {
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

        async move {
            let resp = match http.execute(req).instrument(span.clone()).await {
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
}

impl Env for VmEnv {
    type Error = anyhow::Error;

    fn hash(&self, _update: impl FnMut(&[u8])) {}

    fn send_request_http<B>(
        &self,
        mut request: http::Request<B>,
    ) -> impl Future<
        Output = anyhow::Result<
            http::Response<
                Pin<
                    Box<
                        dyn futures_core::Stream<Item = anyhow::Result<http_body::Frame<Bytes>>>
                            + Send
                            + Sync
                            + 'static,
                    >,
                >,
            >,
        >,
    > + Send
    + 'static
    where
        B: http_body::Body + Send + Sync + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        let span = tracing::span!(
            target: "promptkit::http",
            tracing::Level::INFO,
            "http::fetch",
            promptkit.user = true,
            http.response.body_size = Empty,
            { trace::OTEL_STATUS_CODE } = Empty,
        );
        let _enter = span.enter();

        let s = span.clone();
        let http = self.http.clone();
        async move {
            let mut r = reqwest::Request::new(
                std::mem::take(request.method_mut()),
                reqwest::Url::parse(request.uri().to_string().as_str())?,
            );
            *r.version_mut() = request.version();
            *r.headers_mut() = std::mem::take(request.headers_mut());
            *r.body_mut() = Some(reqwest::Body::from(
                request
                    .into_body()
                    .collect()
                    .await
                    .map_err(Into::<anyhow::Error>::into)?
                    .to_bytes(),
            ));

            let mut resp = Self::send_request(http, r)
                .await
                .map_err(Into::<anyhow::Error>::into)?;

            let mut builder = http::response::Builder::new()
                .status(resp.status())
                .version(resp.version());
            if let Some(h) = builder.headers_mut() {
                *h = std::mem::take(resp.headers_mut());
            }
            let b: Pin<
                Box<
                    dyn futures_core::Stream<Item = anyhow::Result<http_body::Frame<Bytes>>>
                        + Send
                        + Sync
                        + 'static,
                >,
            > = Box::pin(InstrumentStream {
                stream: resp.bytes_stream().map(|f| match f {
                    Ok(d) => Ok(http_body::Frame::data(d)),
                    Err(e) => Err(e.into()),
                }),
                span: s,
                size: 0,
            });
            let b = builder.body(b)?;
            Ok(b)
        }
        .instrument(span.clone())
    }
}

#[pin_project]
struct InstrumentStream<S> {
    #[pin]
    stream: S,
    span: tracing::Span,
    size: usize,
}

impl<S: Stream<Item = Result<http_body::Frame<Bytes>, E>>, E> Stream for InstrumentStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let span = &this.span;
        let enter = span.enter();
        match this.stream.poll_next(cx) {
            Poll::Ready(None) => {
                span.record(trace::OTEL_STATUS_CODE, "OK");
                span.record("http.response.body_size", *this.size as u64);
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(f))) => {
                if let Some(d) = f.data_ref() {
                    *this.size += d.len();
                }
                Poll::Ready(Some(Ok(f)))
            }
            Poll::Ready(Some(Err(e))) => {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
