use std::path::Path;

use axum::extract::FromRef;

use crate::vm_manager::VmManager;

#[derive(Clone)]
pub struct State {
    vm: VmManager,
}

impl State {
    pub fn new(vm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {
            vm: VmManager::new(vm_path.as_ref())?,
        })
    }
}

pub type Router = axum::Router<State>;

impl FromRef<State> for VmManager {
    fn from_ref(app_state: &State) -> Self {
        app_state.vm.clone()
    }
}
