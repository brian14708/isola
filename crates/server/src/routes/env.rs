use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use http::Method;
use http_body_util::Full;
use isola::{
    BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse, OutputSink, WebsocketBodyStream,
    WebsocketRequest, WebsocketResponse,
};
use isola_cbor::{from_cbor, to_cbor};
use isola_request::{RequestContext, RequestOptions, TraceRequest, request_span};
use tokio::sync::mpsc;
use tracing::{field::Empty, level_filters::LevelFilter};

#[derive(Clone)]
pub struct VmEnv {
    pub client: Arc<isola_request::Client>,
    pub log_level: LevelFilter,
}

pub struct Context<F>
where
    F: FnOnce(&TraceRequest<'_>) -> tracing::Span,
{
    make_span: Option<F>,
}

impl<F> RequestContext for Context<F>
where
    F: FnOnce(&TraceRequest<'_>) -> tracing::Span,
{
    fn make_span(&mut self, request: &TraceRequest<'_>) -> tracing::Span {
        self.make_span
            .take()
            .map_or_else(tracing::Span::none, |f| f(request))
    }
}

pub enum StreamItem {
    Data(Bytes),
    End(Option<Bytes>),
    Error(isola::Error),
}

pub struct MpscOutputSink {
    sender: mpsc::Sender<StreamItem>,
}

impl MpscOutputSink {
    #[must_use]
    pub const fn new(sender: mpsc::Sender<StreamItem>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl OutputSink for MpscOutputSink {
    async fn on_partial(&mut self, cbor: Bytes) -> Result<(), BoxError> {
        self.sender
            .send(StreamItem::Data(cbor))
            .await
            .map_err(|_e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "output receiver dropped",
                )) as BoxError
            })
    }

    async fn on_end(&mut self, cbor: Bytes) -> Result<(), BoxError> {
        self.sender
            .send(StreamItem::End(if cbor.is_empty() {
                None
            } else {
                Some(cbor)
            }))
            .await
            .map_err(|_e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "output receiver dropped",
                )) as BoxError
            })
    }
}

#[async_trait]
impl Host for VmEnv {
    async fn hostcall(&self, call_type: &str, payload: Bytes) -> Result<Bytes, BoxError> {
        match call_type {
            "echo" => Ok(payload),
            "add" => {
                #[derive(serde::Deserialize)]
                struct AddInput {
                    a: i32,
                    b: i32,
                }

                let parsed: AddInput = from_cbor(payload.as_ref()).map_err(|e| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)) as BoxError
                })?;
                let result = to_cbor(&(parsed.a + parsed.b)).map_err(|e| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)) as BoxError
                })?;
                Ok(result)
            }
            _ => Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unknown hostcall type",
            ))),
        }
    }

    async fn http_request(&self, request: HttpRequest) -> Result<HttpResponse, BoxError> {
        let mut builder = http::Request::builder()
            .method(request.method)
            .uri(request.uri);

        if let Some(headers) = builder.headers_mut() {
            *headers = request.headers;
        }

        let request = builder
            .body(Full::new(request.body.unwrap_or_default()))
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        let ctx = Context {
            make_span: Some(|r: &TraceRequest<'_>| {
                request_span!(
                    r,
                    target: isola::TRACE_TARGET_SCRIPT,
                    tracing::Level::INFO,
                    "http.request",
                )
            }),
        };

        let response = self
            .client
            .send_http(request, RequestOptions::new(ctx))
            .await
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        Ok(response.map(|body| -> HttpBodyStream {
            Box::pin(
                body.map(|frame| frame.map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)),
            )
        }))
    }

    async fn websocket_connect(
        &self,
        request: WebsocketRequest,
    ) -> Result<WebsocketResponse, BoxError> {
        let mut builder = http::Request::builder()
            .method(Method::GET)
            .uri(request.uri);

        if let Some(headers) = builder.headers_mut() {
            *headers = request.headers;
        }

        let request = builder
            .body(request.outbound)
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        let ctx = Context {
            make_span: Some(|r: &TraceRequest<'_>| {
                request_span!(
                    r,
                    target: isola::TRACE_TARGET_SCRIPT,
                    tracing::Level::INFO,
                    "websocket.connect",
                )
            }),
        };

        let response = self
            .client
            .connect_websocket(request, RequestOptions::new(ctx))
            .await
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        Ok(response.map(|body| -> WebsocketBodyStream {
            Box::pin(
                body.map(|item| item.map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)),
            )
        }))
    }

    fn log_level(&self) -> LevelFilter {
        self.log_level
    }
}
