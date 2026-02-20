use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use isola::{
    BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse, OutputSink,
    request::{Client, RequestOptions, TraceRequest},
    request_span,
};
use tokio::sync::mpsc;
use tracing::field::Empty;

#[derive(Clone)]
pub struct SandboxEnv {
    pub client: Arc<Client>,
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
impl Host for SandboxEnv {
    async fn http_request(&self, incoming: HttpRequest) -> Result<HttpResponse, BoxError> {
        let mut request = http::Request::new(incoming.body().clone().unwrap_or_default());
        *request.method_mut() = incoming.method().clone();
        *request.uri_mut() = incoming.uri().clone();
        *request.headers_mut() = incoming.headers().clone();

        let response = self
            .client
            .send_http(
                request,
                RequestOptions::new().with_make_span(|r: &TraceRequest<'_>| {
                    request_span!(
                        r,
                        target: isola::TRACE_TARGET_SCRIPT,
                        tracing::Level::INFO,
                        "http.request",
                    )
                }),
            )
            .await
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        Ok(response.map(|body| -> HttpBodyStream {
            Box::pin(
                body.map(|frame| frame.map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)),
            )
        }))
    }
}
