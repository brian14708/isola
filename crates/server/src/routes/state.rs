use std::sync::Arc;

use super::{RuntimeFactory, SandboxEnv};
use crate::request::Client;

#[derive(Clone)]
pub struct AppState {
    pub runtime_factory: Arc<RuntimeFactory<SandboxEnv>>,
    pub base_env: SandboxEnv,
}

impl AppState {
    pub async fn new() -> anyhow::Result<Self> {
        let request_proxy = std::env::var("SANDBOX_HTTP_PROXY")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let base_env = SandboxEnv {
            client: Arc::new(Client::new()),
            request_proxy,
        };
        Ok(Self {
            runtime_factory: Arc::new(RuntimeFactory::new().await?),
            base_env,
        })
    }
}
