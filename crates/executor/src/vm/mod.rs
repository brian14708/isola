mod bindgen;
mod host_types;
mod http_client;
mod run;
mod state;

use std::pin::Pin;

pub use bindgen::{exports::promptkit::script::guest_api as exports, Sandbox};
use host_types::ArgumentIterator;
pub use state::VmState;
use tempdir::TempDir;
use tokio::sync::mpsc;
use wasmtime::{component::ResourceTableError, Store};

use crate::{
    trace::BoxedTracer,
    vm::{bindgen::host_api, host_types::HostTypesCtx, run::VmRun},
    ExecStreamItem,
};

pub struct Vm {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState>,
    pub(crate) sandbox: Sandbox,
    pub(crate) workdir: TempDir,
}

impl Vm {
    pub fn run(self, tracer: Option<BoxedTracer>, sender: mpsc::Sender<ExecStreamItem>) -> VmRun {
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
