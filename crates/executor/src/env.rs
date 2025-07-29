use std::{future::Future, pin::Pin};

use bytes::Bytes;
use tokio::task::JoinHandle;

use crate::vm::OutputCallback;

pub struct RpcConnect {
    pub url: String,
    pub metadata: Option<Vec<(String, Vec<u8>)>>,
    pub timeout: Option<std::time::Duration>,
}

pub struct RpcPayload {
    pub data: Vec<u8>,
    pub content_type: Option<String>,
}

pub trait Env {
    type Callback: OutputCallback;
}

pub type BoxedStream<T, E> = Pin<Box<dyn futures::Stream<Item = Result<T, E>> + Send + Sync>>;

pub trait EnvHttp {
    type Error: std::fmt::Display + Send + Sync + 'static;

    fn send_request_http<B>(
        &self,
        _request: http::Request<B>,
    ) -> impl Future<
        Output = Result<
            http::Response<BoxedStream<http_body::Frame<Bytes>, Self::Error>>,
            Self::Error,
        >,
    > + Send
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send;

    fn connect_rpc(
        &self,
        connect: RpcConnect,
        req: tokio::sync::mpsc::Receiver<RpcPayload>,
        resp: tokio::sync::mpsc::Sender<anyhow::Result<RpcPayload>>,
    ) -> impl Future<Output = Result<JoinHandle<anyhow::Result<()>>, Self::Error>> + Send;
}

pub trait EnvHandle: Env + EnvHttp + Send + Clone + 'static {}

impl<T: Env + EnvHttp + Send + Clone + 'static> EnvHandle for T {}
