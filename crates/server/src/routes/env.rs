use std::sync::Arc;

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use promptkit::{BoxedStream, Environment, WebsocketMessage};
use promptkit_cbor::{from_cbor, to_cbor};
use promptkit_request::{RequestContext, RequestOptions, TraceRequest, request_span};
use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tokio::sync::mpsc;
use tracing::{field::Empty, level_filters::LevelFilter};

#[derive(Clone)]
pub struct VmEnv {
    pub client: Arc<promptkit_request::Client>,
    pub log_level: LevelFilter,
}

pub struct Context<F>
where
    F: FnOnce(&TraceRequest) -> tracing::Span,
{
    make_span: Option<F>,
}

impl<F> RequestContext for Context<F>
where
    F: FnOnce(&TraceRequest) -> tracing::Span,
{
    fn make_span(&mut self, r: &TraceRequest) -> tracing::Span {
        self.make_span
            .take()
            .map_or_else(tracing::Span::none, |f| f(r))
    }
}

pub enum StreamItem {
    Data(Bytes),
    End(Option<Bytes>),
    Error(promptkit::Error),
}

pub struct MpscOutputCallback {
    sender: mpsc::Sender<StreamItem>,
}

impl MpscOutputCallback {
    #[must_use]
    pub const fn new(sender: mpsc::Sender<StreamItem>) -> Self {
        Self { sender }
    }
}

impl promptkit::environment::OutputCallback for MpscOutputCallback {
    type Error = std::io::Error;

    async fn on_result(&mut self, item: Bytes) -> Result<(), Self::Error> {
        self.sender
            .send(StreamItem::Data(item))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Send error"))
    }

    async fn on_end(&mut self, item: Bytes) -> Result<(), Self::Error> {
        self.sender
            .send(StreamItem::End(if item.is_empty() {
                None
            } else {
                Some(item)
            }))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Send error"))
    }
}

impl Environment for VmEnv {
    type Error = std::io::Error;
    type Callback = MpscOutputCallback;

    fn log_level(&self) -> LevelFilter {
        self.log_level
    }

    async fn hostcall(&self, call_type: &str, payload: &[u8]) -> Result<Vec<u8>, Self::Error> {
        match call_type {
            "echo" => {
                // Simple echo - return the payload as-is
                Ok(payload.to_vec())
            }
            "add" => {
                #[derive(serde::Deserialize)]
                struct AddInput {
                    a: i32,
                    b: i32,
                }
                let p: AddInput = from_cbor(payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                let result = to_cbor(&(p.a + p.b))
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(result.to_vec())
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unknown hostcall type",
            )),
        }
    }

    async fn http_request<B>(
        &self,
        request: http::Request<B>,
    ) -> std::result::Result<
        http::Response<BoxedStream<http_body::Frame<bytes::Bytes>, Self::Error>>,
        Self::Error,
    >
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        let ctx = Context {
            make_span: Some(|r: &TraceRequest| {
                request_span!(
                    r,
                    target: TRACE_TARGET_SCRIPT,
                    tracing::Level::INFO,
                    "http.request",
                )
            }),
        };
        let http = self.client.http(request, RequestOptions::new(ctx));
        let resp = http.await.map_err(std::io::Error::other)?;
        Ok(resp.map(|b| -> BoxedStream<_, _> { Box::pin(b.map_err(std::io::Error::other)) }))
    }

    async fn websocket_connect<B>(
        &self,
        request: http::Request<B>,
    ) -> Result<http::Response<BoxedStream<WebsocketMessage, Self::Error>>, Self::Error>
    where
        B: futures::Stream<Item = WebsocketMessage> + Sync + Send + 'static,
    {
        let ctx = Context {
            make_span: Some(|r: &TraceRequest| {
                request_span!(
                    r,
                    target: TRACE_TARGET_SCRIPT,
                    tracing::Level::INFO,
                    "websocket.connect",
                )
            }),
        };

        let (parts, body) = request.into_parts();
        Ok(self
            .client
            .websocket(
                http::Request::from_parts(parts, body),
                RequestOptions::new(ctx),
            )
            .await
            .map_err(std::io::Error::other)?
            .map(|b| -> BoxedStream<_, _> {
                Box::pin(b.filter_map(|msg| async {
                    match msg {
                        Ok(s) => Some(Ok(s)),
                        Err(e) => Some(Err(std::io::Error::other(e))),
                    }
                }))
            }))
    }
}
