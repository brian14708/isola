use bytes::Bytes;
use http::{HeaderMap, Method, Uri};
use http_body::Frame;
use std::{pin::Pin, sync::Arc};
use tracing::level_filters::LevelFilter;

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type BoxedStream<T> = Pin<Box<dyn futures::Stream<Item = T> + Send + Sync>>;

pub type HttpBodyStream = BoxedStream<core::result::Result<Frame<Bytes>, BoxError>>;
pub type HttpResponse = http::Response<HttpBodyStream>;

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
}

pub type WebsocketMessage = tungstenite::protocol::Message;
pub type WebsocketBodyStream = BoxedStream<core::result::Result<WebsocketMessage, BoxError>>;
pub type WebsocketResponse = http::Response<WebsocketBodyStream>;
pub type WebsocketOutboundStream = BoxedStream<WebsocketMessage>;

pub struct WebsocketRequest {
    pub uri: Uri,
    pub headers: HeaderMap,
    pub outbound: WebsocketOutboundStream,
}

#[async_trait::async_trait]
pub trait OutputSink: Send + 'static {
    async fn on_partial(&mut self, cbor: Bytes) -> core::result::Result<(), BoxError>;
    async fn on_end(&mut self, cbor: Bytes) -> core::result::Result<(), BoxError>;
}

#[async_trait::async_trait]
pub trait Host: Send + Sync + 'static {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Bytes,
    ) -> core::result::Result<Bytes, BoxError>;

    /// Perform a single-hop HTTP request.
    ///
    /// Safety contract: this MUST NOT follow redirects internally.
    async fn http_request(&self, req: HttpRequest) -> core::result::Result<HttpResponse, BoxError>;

    /// Perform a single WebSocket connect attempt.
    ///
    /// Safety contract: this MUST NOT follow redirects internally.
    async fn websocket_connect(
        &self,
        req: WebsocketRequest,
    ) -> core::result::Result<WebsocketResponse, BoxError>;

    fn log_level(&self) -> LevelFilter {
        LevelFilter::OFF
    }
}

#[async_trait::async_trait]
impl<T: Host + ?Sized> Host for Arc<T> {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Bytes,
    ) -> core::result::Result<Bytes, BoxError> {
        (**self).hostcall(call_type, payload).await
    }

    async fn http_request(&self, req: HttpRequest) -> core::result::Result<HttpResponse, BoxError> {
        (**self).http_request(req).await
    }

    async fn websocket_connect(
        &self,
        req: WebsocketRequest,
    ) -> core::result::Result<WebsocketResponse, BoxError> {
        (**self).websocket_connect(req).await
    }

    fn log_level(&self) -> LevelFilter {
        (**self).log_level()
    }
}
