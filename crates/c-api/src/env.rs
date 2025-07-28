use std::{future::pending, pin::Pin, sync::Arc};

use futures_util::TryStreamExt;
use promptkit_executor::env::HttpResponse;
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
    pub fn shared() -> &'static Env {
        DEFAULT_ENV.get_or_init(Env::default)
    }
}

impl promptkit_executor::Env for Env {
    type Callback = Callback;
    type Error = anyhow::Error;

    fn send_request_http<B>(
        &self,
        request: http::Request<B>,
    ) -> impl Future<Output = std::result::Result<HttpResponse<Self::Error>, Self::Error>> + Send + 'static
    where
        B: http_body::Body + Send + Sync + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        let client = self.client.clone();
        async move {
            let http = client.http(request, RequestOptions::default());
            let resp = http.await.map_err(anyhow::Error::from)?;
            Ok(resp.map(
                |b| -> Pin<
                    Box<
                        dyn futures_util::Stream<
                                Item = anyhow::Result<http_body::Frame<bytes::Bytes>>,
                            > + Send
                            + Sync
                            + 'static,
                    >,
                > { Box::pin(b.map_err(anyhow::Error::from)) },
            ))
        }
    }

    fn connect_rpc(
        &self,
        _connect: promptkit_executor::env::RpcConnect,
        _req: tokio::sync::mpsc::Receiver<promptkit_executor::env::RpcPayload>,
        _resp: tokio::sync::mpsc::Sender<anyhow::Result<promptkit_executor::env::RpcPayload>>,
    ) -> impl Future<
        Output = std::result::Result<tokio::task::JoinHandle<anyhow::Result<()>>, Self::Error>,
    > + Send
    + 'static {
        pending()
    }
}
