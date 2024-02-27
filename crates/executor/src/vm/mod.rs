mod bindgen;
mod host_types;
mod http_client;
mod run;
mod state;

use std::pin::Pin;

pub use bindgen::exports::vm as exports;
pub use bindgen::PythonVm;
use host_types::ArgumentIterator;
pub use state::VmState;

use tokio::sync::mpsc;
use wasmtime::component::ResourceTableError;
use wasmtime::Store;

use crate::trace::BoxedTracer;

use self::bindgen::promptkit::python::types;
use self::host_types::HostTypesCtx;
use self::run::VmRun;

pub struct Vm {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState>,
    pub(crate) python: PythonVm,
}

impl Vm {
    pub fn run(
        self,
        tracer: Option<BoxedTracer>,
        sender: mpsc::Sender<anyhow::Result<(Vec<u8>, bool)>>,
    ) -> VmRun {
        VmRun::new(self, tracer, sender)
    }

    pub fn new_iter(
        &mut self,
        stream: Pin<Box<dyn tokio_stream::Stream<Item = types::Argument> + Send>>,
    ) -> wasmtime::Result<wasmtime::component::Resource<ArgumentIterator>, ResourceTableError> {
        self.store
            .data_mut()
            .table()
            .push(ArgumentIterator::new(stream))
    }
}
