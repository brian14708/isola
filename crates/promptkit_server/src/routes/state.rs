use std::{path::Path, sync::Arc};

use prometheus_client::registry::Registry;

use crate::vm_manager::VmManager;

pub struct AppState {
    pub vm: VmManager,
    pub metrics: Metrics,
}

impl AppState {
    pub fn new(vm_path: impl AsRef<Path>) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            vm: VmManager::new(vm_path.as_ref())?,
            metrics: Metrics::new(),
        }))
    }
}

pub struct Metrics {
    pub registry: Registry,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::default();

        Self { registry }
    }
}
