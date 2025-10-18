use std::sync::Arc;

use futures::TryStreamExt;
use promptkit::{BoxedStream, Environment, WebsocketMessage};
use promptkit_request::{Client, RequestOptions};

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
    pub async fn shared() -> Self {
        DEFAULT_ENV.get_or_init(Self::default).clone()
    }
}

impl Environment for Env {
    type Error = std::io::Error;
    type Callback = crate::Callback;

    async fn hostcall(&self, call_type: &str, payload: &[u8]) -> Result<Vec<u8>, Self::Error> {
        match call_type {
            "echo" => {
                // Simple echo - return the payload as-is
                Ok(payload.to_vec())
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
        let client = self.client.clone();
        let http = client.http(request, RequestOptions::default());
        let resp = http.await.map_err(std::io::Error::other)?;
        Ok(resp.map(|b| -> BoxedStream<_, _> { Box::pin(b.map_err(std::io::Error::other)) }))
    }

    async fn websocket_connect<B>(
        &self,
        _request: http::Request<B>,
    ) -> Result<http::Response<BoxedStream<WebsocketMessage, Self::Error>>, Self::Error>
    where
        B: futures::Stream<Item = WebsocketMessage> + Sync + Send + 'static,
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "websocket not implemented in c-api",
        ))
    }
}
