mod bindgen;
mod run;
mod state;

use std::{
    path::{Path, PathBuf},
    pin::Pin,
};

use bindgen::host::ValueIterator;
pub use bindgen::{Sandbox, SandboxPre, guest as exports};
use bytes::Bytes;
use futures::Stream;
pub use state::{OutputCallback, VmState};
use tempfile::TempDir;
use wasmtime::{Store, component::ResourceTableError};

pub use crate::vm::run::VmRun;
use crate::{env::EnvHandle, vm::bindgen::HostView};

pub struct Vm<E: EnvHandle> {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState<E>>,
    pub(crate) sandbox: Sandbox,
    pub(crate) _workdir: WorkDir,
}

pub enum WorkDir {
    Temp(TempDir),
    Path(PathBuf),
    None,
}

impl WorkDir {
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            WorkDir::Temp(t) => Some(t.path()),
            WorkDir::Path(p) => Some(p.as_ref()),
            WorkDir::None => None,
        }
    }
}

pub type VmIterator = wasmtime::component::Resource<ValueIterator>;

impl<E: EnvHandle> Vm<E> {
    #[must_use]
    pub fn run(self, env: E, callback: E::Callback) -> VmRun<E> {
        VmRun::new(self, env, callback)
    }

    /// Creates a new iterator resource from the given stream.
    ///
    /// # Errors
    ///
    /// Returns an error if the iterator resource cannot be created in the resource table.
    pub fn new_iter(
        &mut self,
        stream: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> wasmtime::Result<VmIterator, ResourceTableError> {
        self.store
            .data_mut()
            .table()
            .push(ValueIterator::new(stream))
    }
}
