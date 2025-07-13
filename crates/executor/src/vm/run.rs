use std::future::Future;

use wasmtime::Store;

use super::{
    Vm, exports,
    state::{OutputCallback, VmRunState, VmState},
};
use crate::Env;

pub struct VmRun<E: Env + 'static> {
    vm: Option<Vm<E>>,
}

impl<E> VmRun<E>
where
    E: Env + 'static,
{
    pub fn new(mut vm: Vm<E>, callback: impl OutputCallback) -> Self {
        let o: &mut VmState<_> = vm.store.data_mut();
        o.run = Some(VmRunState {
            output: Box::new(callback),
        });
        Self { vm: Some(vm) }
    }

    pub async fn exec<'a, F, Output>(
        &'a mut self,
        f: impl FnOnce(&'a exports::Guest, &'a mut Store<VmState<E>>) -> F + Send,
    ) -> Output
    where
        F: Future<Output = Output> + Send,
    {
        let vm = self.vm.as_mut().unwrap();
        f(vm.sandbox.promptkit_script_guest(), &mut vm.store).await
    }

    fn cleanup(&mut self) {
        if let Some(vm) = self.vm.as_mut() {
            let o: &mut VmState<_> = vm.store.data_mut();
            o.run = None;
        }
    }

    pub fn reuse(mut self) -> Vm<E> {
        self.cleanup();
        self.vm.take().unwrap()
    }
}

impl<E> Drop for VmRun<E>
where
    E: Env,
{
    fn drop(&mut self) {
        self.cleanup();
    }
}
