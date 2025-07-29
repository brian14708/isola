use std::{pin::Pin, sync::Arc};

use futures::{Stream, TryStreamExt};
use promptkit_executor::env::BoxedStream;
use promptkit_request::{Client, RequestOptions};

use crate::Callback;

#[derive(Clone)]
pub struct Env {
    pub client: Arc<promptkit_request::Client>,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            client: Arc::new(Client::new()),
        }
    }
}

static DEFAULT_ENV: std::sync::OnceLock<Env> = std::sync::OnceLock::new();

impl Env {
    #[expect(clippy::unused_async, reason = "env must be created in async context")]
    pub async fn shared() -> Env {
        DEFAULT_ENV.get_or_init(Env::default).clone()
    }
}

impl promptkit_executor::env::Env for Env {
    type Callback = Callback;
}
impl promptkit_executor::env::EnvHttp for Env {
    type Error = anyhow::Error;

    async fn send_request_http<B>(
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
        let client = self.client.clone();
        let http = client.http(request, RequestOptions::default());
        let resp = http.await.map_err(anyhow::Error::from)?;
        Ok(resp.map(
            |b| -> Pin<
                Box<
                    dyn Stream<Item = Result<http_body::Frame<bytes::Bytes>, anyhow::Error>>
                        + Send
                        + Sync
                        + 'static,
                >,
            > { Box::pin(b.map_err(anyhow::Error::from)) },
        ))
    }

    async fn connect_rpc(
        &self,
        _connect: promptkit_executor::env::RpcConnect,
        _req: tokio::sync::mpsc::Receiver<promptkit_executor::env::RpcPayload>,
        _resp: tokio::sync::mpsc::Sender<anyhow::Result<promptkit_executor::env::RpcPayload>>,
    ) -> std::result::Result<tokio::task::JoinHandle<anyhow::Result<()>>, Self::Error> {
        unimplemented!()
    }
}
