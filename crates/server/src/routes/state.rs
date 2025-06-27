use std::{path::Path, sync::Arc};

use axum::extract::FromRef;
use promptkit_executor::VmManager;

use super::env::VmEnv;

#[derive(Clone)]
pub struct AppState {
    pub vm: Arc<VmManager<VmEnv>>,
}

impl AppState {
    pub async fn new(vm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {
            vm: Arc::new(VmManager::<VmEnv>::new(vm_path.as_ref()).await?),
        })
    }
}

impl FromRef<AppState> for Arc<VmManager<VmEnv>> {
    fn from_ref(state: &AppState) -> Self {
        state.vm.clone()
    }
}
