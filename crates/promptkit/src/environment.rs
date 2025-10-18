use bytes::Bytes;
use http_body::Frame;
use std::pin::Pin;
use tracing::level_filters::LevelFilter;

pub type BoxedStream<T, E> = Pin<Box<dyn futures::Stream<Item = Result<T, E>> + Send + Sync>>;
type HttpResponse<E> = Result<http::Response<BoxedStream<Frame<Bytes>, E>>, E>;
pub type WebsocketMessage = tungstenite::protocol::Message;
type WebsocketResponse<E> = Result<http::Response<BoxedStream<WebsocketMessage, E>>, E>;

pub trait OutputCallback: Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn on_result(&mut self, item: Bytes) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn on_end(&mut self, item: Bytes) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait Environment: Clone + Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    type Callback: OutputCallback;

    fn hostcall(
        &self,
        call_type: &str,
        payload: &[u8],
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send;

    fn http_request<B>(
        &self,
        _request: http::Request<B>,
    ) -> impl std::future::Future<Output = HttpResponse<Self::Error>> + Send
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send;

    fn websocket_connect<B>(
        &self,
        _request: http::Request<B>,
    ) -> impl std::future::Future<Output = WebsocketResponse<Self::Error>> + Send
    where
        B: futures::Stream<Item = WebsocketMessage> + Send + Sync + 'static;

    fn log_level(&self) -> LevelFilter {
        LevelFilter::OFF
    }
}
