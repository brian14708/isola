mod bindgen;
mod http_client;
mod run;
mod state;

pub use bindgen::exports::vm as exports;
pub use bindgen::PythonVm;
pub use state::VmState;

use tokio::sync::mpsc;
use wasmtime::Store;

use crate::trace::Tracer;

use self::run::VmRun;

pub struct Vm {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState>,
    pub(crate) python: PythonVm,
}

impl Vm {
    pub fn run<T>(
        self,
        tracer: Option<T>,
        sender: mpsc::Sender<anyhow::Result<(String, bool)>>,
    ) -> VmRun
    where
        T: Tracer,
    {
        VmRun::new(self, tracer, sender)
    }
}
