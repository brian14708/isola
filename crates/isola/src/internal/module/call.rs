use wasmtime::Store;

use crate::{
    host::{Host, OutputTarget},
    internal::sandbox::InstanceState,
};

/// RAII guard that clears the output target when dropped, even if the call
/// panics or returns early.
pub struct CallCleanup<'a, H: Host> {
    pub store: &'a mut Store<InstanceState<H>>,
}

impl<'a, H: Host> CallCleanup<'a, H> {
    pub const fn new(store: &'a mut Store<InstanceState<H>>) -> Self {
        Self { store }
    }

    pub fn set_output_target(&mut self, target: OutputTarget) {
        self.store.data_mut().set_output_target(Some(target));
    }
}

impl<H: Host> Drop for CallCleanup<'_, H> {
    fn drop(&mut self) {
        // Cleanup only; explicit flush is handled by call sites.
        self.store.data_mut().set_output_target(None);
    }
}

impl<H: Host> std::ops::Deref for CallCleanup<'_, H> {
    type Target = Store<InstanceState<H>>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl<H: Host> std::ops::DerefMut for CallCleanup<'_, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl<H: Host> wasmtime::AsContext for CallCleanup<'_, H> {
    type Data = InstanceState<H>;

    fn as_context(&self) -> wasmtime::StoreContext<'_, Self::Data> {
        wasmtime::AsContext::as_context(&*self.store)
    }
}

impl<H: Host> wasmtime::AsContextMut for CallCleanup<'_, H> {
    fn as_context_mut(&mut self) -> wasmtime::StoreContextMut<'_, Self::Data> {
        wasmtime::AsContextMut::as_context_mut(&mut *self.store)
    }
}
