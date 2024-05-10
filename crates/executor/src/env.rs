use std::future::Future;
use std::sync::Arc;

use promptkit_llm::tokenizers::Tokenizer;
use thiserror::Error;

pub trait Env {
    fn hash(&self, update: impl FnMut(&[u8]));

    fn send_request(
        &self,
        _request: reqwest::Request,
    ) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send + 'static;

    fn get_tokenizer(
        &self,
        _name: &str,
    ) -> impl Future<Output = Result<Arc<dyn Tokenizer + Send + Sync>, EnvError>> + Send {
        async { Err(EnvError::Unimplemented) }
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
