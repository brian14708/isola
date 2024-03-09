mod bindgen;
mod host_types;
mod http_client;
mod run;
mod state;

use std::pin::Pin;

pub use bindgen::exports::promptkit::script::guest_api as exports;
pub use bindgen::Sandbox;
use host_types::ArgumentIterator;
pub use state::VmState;

use tokio::sync::mpsc;
use wasmtime::component::ResourceTableError;
use wasmtime::Store;

use crate::trace::BoxedTracer;

use self::bindgen::host_api;
use self::host_types::HostTypesCtx;
use self::run::VmRun;

pub struct Vm {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState>,
    pub(crate) sandbox: Sandbox,
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
        stream: Pin<Box<dyn tokio_stream::Stream<Item = host_api::Argument> + Send>>,
    ) -> wasmtime::Result<wasmtime::component::Resource<ArgumentIterator>, ResourceTableError> {
        self.store
            .data_mut()
            .table()
            .push(ArgumentIterator::new(stream))
    }
}
