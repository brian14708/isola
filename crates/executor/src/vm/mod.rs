mod bindgen;
mod run;
mod state;

use bindgen::host::ValueIterator;
pub use bindgen::{Sandbox, SandboxPre, guest as exports};
use futures_core::Stream;
pub use state::{OutputCallback, VmState};
use tempfile::TempDir;
use wasmtime::{Store, component::ResourceTableError};
use wasmtime_wasi::p2::IoView;

pub use crate::{Env, vm::run::VmRun};

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
    #[must_use]
    pub fn run(self, callback: impl OutputCallback) -> VmRun<E> {
        VmRun::new(self, callback)
    }

    /// Creates a new iterator resource from the given stream.
    ///
    /// # Errors
    ///
    /// Returns an error if the iterator resource cannot be created in the resource table.
    pub fn new_iter(
        &mut self,
        stream: impl Stream<Item = Vec<u8>> + Send + 'static,
    ) -> wasmtime::Result<wasmtime::component::Resource<ValueIterator>, ResourceTableError> {
        self.store
            .data_mut()
            .table()
            .push(ValueIterator::new(stream))
    }
}
