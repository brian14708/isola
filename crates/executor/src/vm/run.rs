use std::future::Future;

use wasmtime::Store;

use super::{
    Vm, exports,
    state::{VmRunState, VmState},
};
use crate::Env;

pub struct VmRun<E: Env> {
    vm: Vm<E>,
}

impl<E: Env> VmRun<E> {
    pub fn new(mut vm: Vm<E>, callback: E::Callback) -> Self {
        let o: &mut VmState<_> = vm.store.data_mut();
        o.run = Some(VmRunState { output: callback });
        Self { vm }
    }

    pub async fn exec<'a, F, Output>(
        &'a mut self,
        f: impl FnOnce(&'a exports::Guest, &'a mut Store<VmState<E>>) -> F + Send,
    ) -> Output
    where
        F: Future<Output = Output> + Send,
    {
        f(self.vm.sandbox.promptkit_script_guest(), &mut self.vm.store).await
    }

    #[must_use]
    pub fn reuse(mut self) -> Vm<E> {
        let o: &mut VmState<_> = self.vm.store.data_mut();
        o.run = None;
        self.vm
    }
}
