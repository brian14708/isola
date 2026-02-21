use std::sync::Arc;

use async_trait::async_trait;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse},
    value::Value,
};
use isola_request::{Client, RequestOptions};
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct Env {
    client: Arc<Client>,
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
    async fn hostcall(&self, call_type: &str, payload: Value) -> Result<Value, BoxError> {
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

    async fn http_request(&self, incoming: HttpRequest) -> Result<HttpResponse, BoxError> {
        let mut request = http::Request::new(incoming.body().clone().unwrap_or_default());
        *request.method_mut() = incoming.method().clone();
        *request.uri_mut() = incoming.uri().clone();
        *request.headers_mut() = incoming.headers().clone();

        let response = self
            .client
            .send_http(request, RequestOptions::default())
            .await
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        Ok(response.map(|body| -> HttpBodyStream {
            Box::pin(
                body.map(|frame| frame.map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)),
            )
        }))
    }
}
