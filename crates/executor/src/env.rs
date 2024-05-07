use std::sync::Arc;

use promptkit_llm::tokenizers::Tokenizer;
use thiserror::Error;

#[async_trait::async_trait]
pub trait Env {
    fn hash(&self, update: impl FnMut(&[u8]));

    async fn send_request(&self, request: reqwest::Request) -> Result<reqwest::Response, EnvError>;

    async fn get_tokenizer(
        &self,
        _name: &str,
    ) -> Result<Arc<dyn Tokenizer + Send + Sync>, EnvError> {
        Err(EnvError::Unimplemented)
    }
}

#[derive(Error, Debug)]
pub enum EnvError {
    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Not found")]
    NotFound,

    #[error("Internal error")]
    Internal(anyhow::Error),

    #[error("Unimplemented")]
    Unimplemented,
}
