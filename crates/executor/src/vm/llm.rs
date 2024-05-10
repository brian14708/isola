use std::sync::Arc;

use futures_util::Future;
use promptkit_llm::tokenizers::{DecodeOption, EncodeOption, Tokenizer as LlmTokenizer};
use tracing::{field::Empty, span};
use wasmtime_wasi::ResourceTable;

use super::bindgen::promptkit::script::llm;

pub trait LlmView: Send {
    fn table(&mut self) -> &mut ResourceTable;

    fn get_tokenizer(
        &self,
        name: &str,
    ) -> impl Future<Output = Option<Arc<dyn LlmTokenizer + Send + Sync>>> + Send;
}

#[async_trait::async_trait]
impl<I: LlmView> llm::Host for I {}

#[async_trait::async_trait]
impl<I: LlmView> llm::HostTokenizer for I {
    async fn new(
        &mut self,
        name: String,
    ) -> wasmtime::Result<wasmtime::component::Resource<Tokenizer>> {
        let tokenizer = self
            .get_tokenizer(&name)
            .await
            .ok_or_else(|| anyhow::anyhow!("tokenizer not found"))?;
        Ok(self.table().push(Tokenizer { inner: tokenizer })?)
    }

    async fn encode(
        &mut self,
        tokenizer: wasmtime::component::Resource<Tokenizer>,
        data: String,
    ) -> wasmtime::Result<Vec<u32>> {
        let span = span!(
            target: "promptkit::llm",
            tracing::Level::INFO,
            "llm::tokenizer::encode",
            promptkit.user = true,
            llm.tokenizer.input_len = data.len(),
            llm.tokenizer.token_count = Empty,
        );
        let _guard = span.enter();
        let tokenizer = self.table().get(&tokenizer)?;
        let ids = tokenizer.inner.encode(&data, &EncodeOption {})?.ids;
        span.record("llm.tokenizer.token_count", ids.len());
        Ok(ids)
    }

    async fn decode(
        &mut self,
        tokenizer: wasmtime::component::Resource<Tokenizer>,
        tokens: Vec<u32>,
    ) -> wasmtime::Result<String> {
        let span = span!(
            target: "promptkit::llm",
            tracing::Level::INFO,
            "llm::tokenizer::decode",
            promptkit.user = true,
            llm.tokenizer.token_count = tokens.len(),
            llm.tokenizer.output_len = Empty,
        );
        let _guard = span.enter();
        let tokenizer = self.table().get(&tokenizer)?;
        let out = tokenizer.inner.decode(&tokens, &DecodeOption {})?;
        span.record("llm.tokenizer.output_len", out.len());
        Ok(out)
    }

    async fn get_special_token(
        &mut self,
        tokenizer: wasmtime::component::Resource<Tokenizer>,
        name: String,
    ) -> wasmtime::Result<Option<u32>> {
        let tokenizer = self.table().get(&tokenizer)?;
        Ok(tokenizer.inner.special_token(&name))
    }

    fn drop(
        &mut self,
        tokenizer: wasmtime::component::Resource<Tokenizer>,
    ) -> wasmtime::Result<()> {
        self.table().delete(tokenizer)?;
        Ok(())
    }
}

pub struct Tokenizer {
    inner: Arc<dyn LlmTokenizer + Send + Sync>,
}
