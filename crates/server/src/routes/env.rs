use std::{
    borrow::Cow,
    future::Future,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};

use anyhow::anyhow;
use bytes::Bytes;
use futures_core::Stream;
use futures_util::{StreamExt, TryFutureExt, TryStreamExt, stream};
use http::{HeaderName, HeaderValue, Uri};
use http_body_util::BodyExt;
use http_cache_reqwest::CacheMode;
use opentelemetry_semantic_conventions::attribute as trace;
use pin_project::pin_project;
use reqwest_middleware::ClientWithMiddleware;
use tokio::task::JoinHandle;
use tracing::{Instrument, field::Empty};

use promptkit_executor::{
    Env,
    env::{RpcConnect, RpcPayload},
};
use tokio_tungstenite::tungstenite::{
    self, ClientRequestBuilder,
    client::IntoClientRequest,
    protocol::{CloseFrame, frame::coding::CloseCode},
};
use url::Url;

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
    ) -> impl std::future::Future<
        Output = reqwest_middleware::Result<(tracing::Span, reqwest::Response)>,
    > + Send
    + 'static {
        let span = tracing::span!(
            target: "promptkit::http",
            tracing::Level::INFO,
            "http::fetch",
            promptkit.user = true,
            otel.kind = "client",
            { trace::HTTP_REQUEST_METHOD } = req.method().as_str(),
            { trace::SERVER_ADDRESS } = req.url().host_str().unwrap_or_default(),
            { trace::SERVER_PORT } = req.url().port_or_known_default().unwrap_or_default(),
            { trace::URL_FULL } = req.url().to_string(),
            { trace::HTTP_RESPONSE_STATUS_CODE } = Empty,
            { trace::HTTP_RESPONSE_BODY_SIZE }= Empty,
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
            span.record(trace::HTTP_RESPONSE_STATUS_CODE, status.as_u16());
            if status.is_server_error() || status.is_client_error() {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
            }
            Ok((span, resp))
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

            let (span, mut resp) = Self::send_request(http, r)
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
                span,
                size: 0,
            });
            let b = builder.body(b)?;
            Ok(b)
        }
        .in_current_span()
    }

    fn connect_rpc(
        &self,
        connect: RpcConnect,
        req: tokio::sync::mpsc::Receiver<RpcPayload>,
        resp: tokio::sync::mpsc::Sender<RpcPayload>,
    ) -> impl Future<Output = Result<JoinHandle<anyhow::Result<()>>, Self::Error>> + Send + 'static
    {
        let url = Url::parse(&connect.url).unwrap();
        let http = self.http.clone();
        async move {
            let timeout = connect.timeout;
            if url.scheme() == "ws" || url.scheme() == "wss" {
                let fut = websocket(http, connect, req, resp);
                if let Some(d) = timeout {
                    tokio::time::timeout(d, fut)
                        .await
                        .unwrap_or_else(|_| Err(anyhow!("timeout")))
                } else {
                    fut.await
                }
            } else {
                Err(anyhow!("unsupported protocol"))
            }
        }
        .in_current_span()
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
                span.record(trace::HTTP_RESPONSE_BODY_SIZE, *this.size as u64);
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

#[allow(clippy::too_many_lines)]
async fn websocket(
    http: ClientWithMiddleware,
    connect: RpcConnect,
    req: tokio::sync::mpsc::Receiver<RpcPayload>,
    resp: tokio::sync::mpsc::Sender<RpcPayload>,
) -> anyhow::Result<JoinHandle<anyhow::Result<()>>> {
    let mut u = Url::parse(&connect.url)?;
    if u.scheme() == "ws" {
        u.set_scheme("http").unwrap();
    } else if u.scheme() == "wss" {
        u.set_scheme("https").unwrap();
    }

    let (span, ws) = async {
        let span = tracing::span!(
            target: "promptkit::rpc",
            tracing::Level::INFO,
            "rpc::websocket::connect",
            promptkit.user = true,
            otel.kind = "client",
            { trace::HTTP_REQUEST_METHOD } = "GET",
            { trace::SERVER_ADDRESS } = u.host_str().unwrap_or_default(),
            { trace::SERVER_PORT } = u.port_or_known_default().unwrap_or_default(),
            { trace::URL_FULL } = u.to_string(),
            { trace::HTTP_RESPONSE_STATUS_CODE } = Empty,
            { trace::OTEL_STATUS_CODE } = Empty,
        );

        let mut r = ClientRequestBuilder::new(u.to_string().parse::<Uri>().unwrap())
            .into_client_request()?;
        opentelemetry::global::get_text_map_propagator(|injector| {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            struct RequestCarrier<'a> {
                request: &'a mut http::Request<()>,
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
            injector.inject_context(&context, &mut RequestCarrier { request: &mut r });
        });
        let conn = http
            .get(u)
            .headers(r.headers().clone())
            .with_extension(CacheMode::NoStore)
            .send()
            .and_then(|resp| async {
                span.record(trace::HTTP_RESPONSE_STATUS_CODE, resp.status().as_u16());
                Ok(resp.upgrade().await?)
            })
            .and_then(|response| async {
                Ok(tokio_tungstenite::WebSocketStream::from_raw_socket(
                    response,
                    tungstenite::protocol::Role::Client,
                    None,
                )
                .await)
            })
            .await;
        span.record(
            trace::OTEL_STATUS_CODE,
            if conn.is_ok() { "OK" } else { "ERROR" },
        );
        Ok::<_, anyhow::Error>((span, conn?))
    }
    .await?;
    let (tx, rx) = ws.split();

    let s = span.clone();
    Ok(tokio::spawn(
        (async move {
            let write_task = tokio_stream::wrappers::ReceiverStream::new(req)
                .map(|msg| {
                    Ok(if msg.content_type.is_some_and(|t| t.starts_with("text")) {
                        tungstenite::Message::Text(String::from_utf8(msg.data).unwrap().into())
                    } else {
                        tungstenite::Message::Binary(msg.data.into())
                    })
                })
                .chain(stream::iter([Ok(tungstenite::Message::Close(Some(
                    CloseFrame {
                        code: CloseCode::Normal,
                        reason: "".into(),
                    },
                )))]))
                .forward(tx)
                .map_err(anyhow::Error::new);

            let read_task = tokio_stream::StreamExt::map(rx, |msg| match msg {
                Ok(tungstenite::Message::Text(t)) => Ok(Some(RpcPayload {
                    content_type: Some("text/plain".into()),
                    data: t.to_string().into_bytes(),
                })),
                Ok(tungstenite::Message::Binary(b)) => Ok(Some(RpcPayload {
                    content_type: None,
                    data: b.to_vec(),
                })),
                Ok(
                    tungstenite::Message::Close(_)
                    | tungstenite::Message::Ping(_)
                    | tungstenite::Message::Pong(_)
                    | tungstenite::Message::Frame(_),
                ) => Ok(None),
                Err(_) => Err(anyhow!("Error recv message")),
            })
            .then(|msg| {
                let resp = resp.clone();
                async move {
                    match msg {
                        Ok(Some(t)) => Ok(resp.send(t).await?),
                        Ok(None) => Ok(()),
                        Err(e) => Err(e),
                    }
                }
            })
            .try_collect::<()>();

            futures_util::future::try_join(read_task, write_task).await?;
            s.record(trace::OTEL_STATUS_CODE, "OK");
            Ok(())
        })
        .instrument(span),
    ))
}
