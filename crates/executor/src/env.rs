use std::{future::Future, pin::Pin};

use bytes::Bytes;
use http_body::Frame;

use crate::vm::OutputCallback;

pub type WebsocketMessage = tungstenite::protocol::Message;

pub trait Env {
    type Callback: OutputCallback;
}

pub type BoxedStream<T, E> = Pin<Box<dyn futures::Stream<Item = Result<T, E>> + Send + Sync>>;

type HttpBodyStream<E> = BoxedStream<Frame<Bytes>, E>;

pub trait EnvHttp {
    type Error: std::fmt::Display + Send + Sync + 'static;

    fn send_request_http<B>(
        &self,
        _request: http::Request<B>,
    ) -> impl Future<Output = Result<http::Response<HttpBodyStream<Self::Error>>, Self::Error>> + Send
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send;

    fn connect_websocket<B>(
        &self,
        request: http::Request<B>,
    ) -> impl std::future::Future<
        Output = Result<http::Response<BoxedStream<WebsocketMessage, Self::Error>>, Self::Error>,
    > + Send
    where
        B: futures::Stream<Item = WebsocketMessage> + Send + Sync + 'static;
}

pub trait EnvHandle: Env + EnvHttp + Send + Clone + 'static {}

impl<T: Env + EnvHttp + Send + Clone + 'static> EnvHandle for T {}
