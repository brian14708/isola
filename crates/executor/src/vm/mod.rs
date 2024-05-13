mod bindgen;
mod run;
mod state;

use std::pin::Pin;

pub use bindgen::{exports::promptkit::vm::guest as exports, Sandbox};
pub use state::VmState;
use tempdir::TempDir;
use tokio::sync::mpsc;
use wasmtime::{component::ResourceTableError, Store};

use crate::{vm::run::VmRun, Env, ExecStreamItem};

use crate::wasm::vm::{bindings::host::Argument, types::ArgumentIterator, VmView};

pub struct Vm<E> {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState<E>>,
    pub(crate) sandbox: Sandbox,
    pub(crate) workdir: TempDir,
}

impl<E> Vm<E>
where
    E: Env + Send,
{
    pub fn run(self, sender: mpsc::Sender<ExecStreamItem>) -> VmRun<E> {
        VmRun::new(self, sender)
    }

    pub fn new_iter(
        &mut self,
        stream: Pin<Box<dyn tokio_stream::Stream<Item = Argument> + Send>>,
    ) -> wasmtime::Result<wasmtime::component::Resource<ArgumentIterator>, ResourceTableError> {
        self.store
            .data_mut()
            .table()
            .push(ArgumentIterator::new(stream))
    }
}
