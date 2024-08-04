use std::future::Future;

use thiserror::Error;

pub trait Env {
    fn hash(&self, update: impl FnMut(&[u8]));

    fn send_request(
        &self,
        _request: reqwest::Request,
    ) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send + 'static;
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
