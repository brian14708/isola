mod bindgen;
mod run;
mod state;

use std::pin::Pin;

use bindgen::host::{Value, ValueIterator};
pub use bindgen::{Sandbox, SandboxPre, guest as exports};
pub use state::{OutputCallback, VmState};
use tempfile::TempDir;
use wasmtime::{Store, component::ResourceTableError};
use wasmtime_wasi::p2::IoView;

use crate::{Env, vm::run::VmRun};

pub struct Vm<E: 'static> {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState<E>>,
    pub(crate) sandbox: Sandbox,
    pub(crate) workdir: TempDir,
}

impl<E> Vm<E>
where
    E: Env + Send + 'static,
{
    pub fn run(self, callback: impl OutputCallback) -> VmRun<E> {
        VmRun::new(self, callback)
    }

    pub fn new_iter(
        &mut self,
        stream: Pin<Box<dyn tokio_stream::Stream<Item = Value> + Send>>,
    ) -> wasmtime::Result<wasmtime::component::Resource<ValueIterator>, ResourceTableError> {
        self.store
            .data_mut()
            .table()
            .push(ValueIterator::new(stream))
    }
}
