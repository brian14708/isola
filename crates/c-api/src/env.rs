use async_trait::async_trait;
use bytes::Bytes;
use isola::{BoxError, Host};

#[derive(Clone, Default)]
pub struct Env;

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
}
