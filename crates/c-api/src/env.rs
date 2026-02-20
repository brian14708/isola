use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::Full;
use isola::{
    BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse,
    request::{Client, RequestOptions},
};

#[derive(Clone)]
pub struct Env {
    pub client: Arc<Client>,
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

#[async_trait]
impl Host for Env {
    async fn hostcall(&self, call_type: &str, payload: Bytes) -> Result<Bytes, BoxError> {
        match call_type {
            "echo" => {
                // Simple echo - return the payload as-is
                Ok(payload)
            }
            _ => Err(
                std::io::Error::new(std::io::ErrorKind::Unsupported, "unknown hostcall type")
                    .into(),
            ),
        }
    }

    async fn http_request(&self, req: HttpRequest) -> std::result::Result<HttpResponse, BoxError> {
        let client = self.client.clone();
        let mut request = http::Request::new(Full::new(req.body.unwrap_or_default()));
        *request.method_mut() = req.method;
        *request.uri_mut() = req.uri;
        *request.headers_mut() = req.headers;

        let http = client.send_http(request, RequestOptions::default());
        let resp = http.await.map_err(|e| -> BoxError { Box::new(e) })?;
        Ok(
            resp.map(|b| -> HttpBodyStream {
                Box::pin(b.map_err(|e| -> BoxError { Box::new(e) }))
            }),
        )
    }
}
