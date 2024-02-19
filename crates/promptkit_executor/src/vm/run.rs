use std::future::Future;

use tokio::sync::mpsc;
use wasmtime::Store;

use crate::trace::Tracer;

use super::{exports, Vm, VmRunState, VmState};

pub struct VmRun {
    vm: Option<Vm>,
}

impl VmRun {
    pub fn new<T>(
        mut vm: Vm,
        tracer: Option<T>,
        sender: mpsc::Sender<anyhow::Result<(String, bool)>>,
    ) -> Self
    where
        T: Tracer,
    {
        let o: &mut VmState = vm.store.data_mut();
        if let Some(tracer) = tracer {
            // SAFETY: no other reference to tracer is in use
            unsafe { o.tracer.set_unguarded(Some(tracer.boxed())) };
        }
        o.run = Some(VmRunState { output: sender });
        Self { vm: Some(vm) }
    }

    pub async fn exec<'a, F, Output>(
        &'a mut self,
        f: impl FnOnce(&'a exports::Vm, &'a mut Store<VmState>) -> F,
    ) -> Output
    where
        F: Future<Output = Output>,
    {
        let vm = self.vm.as_mut().unwrap();
        f(vm.python.vm(), &mut vm.store).await
    }

    fn cleanup(&mut self) {
        if let Some(vm) = self.vm.as_mut() {
            let o: &mut VmState = vm.store.data_mut();
            o.run = None;
            // SAFETY: no other reference to tracer is in use
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
