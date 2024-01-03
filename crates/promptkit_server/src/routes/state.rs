use std::{path::Path, sync::Arc};

use axum::extract::FromRef;
use prometheus_client::registry::Registry;

use crate::vm_manager::VmManager;

#[derive(Clone)]
pub struct AppState {
    pub vm: Arc<VmManager>,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(vm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {
            vm: Arc::new(VmManager::new(vm_path.as_ref())?),
            metrics: Arc::new(Metrics::default()),
        })
    }
}

impl FromRef<AppState> for Arc<VmManager> {
    fn from_ref(state: &AppState) -> Self {
        state.vm.clone()
    }
}

impl FromRef<AppState> for Arc<Metrics> {
    fn from_ref(state: &AppState) -> Self {
        state.metrics.clone()
    }
}

pub struct Metrics {
    pub registry: Registry,
}

impl Default for Metrics {
    fn default() -> Self {
        let registry = Registry::default();

        Self { registry }
    }
}
