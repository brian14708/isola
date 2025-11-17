use std::{path::Path, sync::Arc};

use axum::extract::FromRef;
use tracing::level_filters::LevelFilter;

use super::{VmEnv, VmManager};

#[derive(Clone)]
pub struct AppState {
    pub vm: Arc<VmManager<VmEnv>>,
    pub base_env: VmEnv,
}

impl AppState {
    pub async fn new(vm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_env = VmEnv {
            client: Arc::new(promptkit_request::Client::new()),
            log_level: LevelFilter::OFF,
        };
        Ok(Self {
            vm: Arc::new(VmManager::new(vm_path.as_ref()).await?),
            base_env,
        })
    }
}

impl FromRef<AppState> for Arc<VmManager<VmEnv>> {
    fn from_ref(state: &AppState) -> Self {
        state.vm.clone()
    }
}
