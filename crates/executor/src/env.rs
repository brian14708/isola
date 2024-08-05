use std::sync::Arc;
use std::{future::Future, pin::Pin};

use bytes::Bytes;
use promptkit_llm::tokenizers::Tokenizer;

pub trait Env {
    type Error: std::fmt::Display + Send + Sync + 'static;

    fn hash(&self, update: impl FnMut(&[u8]));

    fn send_request_http<B>(
        &self,
        _request: http::Request<B>,
    ) -> impl Future<
        Output = Result<
            http::Response<
                Pin<
                    Box<
                        dyn futures_core::Stream<
                                Item = Result<http_body::Frame<Bytes>, Self::Error>,
                            > + Send
                            + Sync
                            + 'static,
                    >,
                >,
            >,
            Self::Error,
        >,
    > + Send
           + 'static
    where
        B: http_body::Body + Send + Sync + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send;

    fn get_tokenizer(
        &self,
        _name: &str,
    ) -> impl Future<Output = Result<Arc<dyn Tokenizer + Send + Sync>, Self::Error>> + Send;
}
