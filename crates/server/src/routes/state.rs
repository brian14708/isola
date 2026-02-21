use std::{path::Path, sync::Arc};

use axum::extract::FromRef;
use isola_request::Client;

use super::{SandboxEnv, SandboxManager};

#[derive(Clone)]
pub struct AppState {
    pub sandbox_manager: Arc<SandboxManager<SandboxEnv>>,
    pub base_env: SandboxEnv,
}

impl AppState {
    pub async fn new(wasm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_env = SandboxEnv {
            client: Arc::new(Client::new()),
        };
        Ok(Self {
            sandbox_manager: Arc::new(SandboxManager::new(wasm_path.as_ref()).await?),
            base_env,
        })
    }
}

impl FromRef<AppState> for Arc<SandboxManager<SandboxEnv>> {
    fn from_ref(state: &AppState) -> Self {
        state.sandbox_manager.clone()
    }
}
