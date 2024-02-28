use std::future::Future;

use tokio::sync::mpsc;
use wasmtime::Store;

use crate::trace::BoxedTracer;

use super::{
    exports,
    state::{VmRunState, VmState},
    Vm,
};

pub struct VmRun {
    vm: Option<Vm>,
}

impl VmRun {
    pub fn new(
        mut vm: Vm,
        tracer: Option<BoxedTracer>,
        sender: mpsc::Sender<anyhow::Result<(Vec<u8>, bool)>>,
    ) -> Self {
        let o: &mut VmState = vm.store.data_mut();
        if let Some(tracer) = tracer {
            // SAFETY: vm is not running yet
            unsafe { o.tracer.set_unguarded(Some(tracer)) };
        }
        o.run = Some(VmRunState { output: sender });
        Self { vm: Some(vm) }
    }

    pub async fn exec<'a, F, Output>(
        &'a mut self,
        f: impl FnOnce(&'a exports::Guest, &'a mut Store<VmState>) -> F + Send,
    ) -> Output
    where
        F: Future<Output = Output> + Send,
    {
        let vm = self.vm.as_mut().unwrap();
        f(vm.python.guest(), &mut vm.store).await
    }

    fn cleanup(&mut self) {
        if let Some(vm) = self.vm.as_mut() {
            let o: &mut VmState = vm.store.data_mut();
            o.run = None;
            // SAFETY: vm is not running anymore
            unsafe { o.tracer.set_unguarded(None) };
        }
    }

    pub fn reuse(mut self) -> Vm {
        self.cleanup();
        self.vm.take().unwrap()
    }
}

impl Drop for VmRun {
    fn drop(&mut self) {
        self.cleanup();
    }
}
