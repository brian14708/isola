wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../wit",
    imports: {
        default: async | trappable,
    },
    exports: {
        default: async | trappable,
    },
    ownership: Borrowing {
        duplicate_if_necessary: true
    },
    with: {
        "wasi:io": wasmtime_wasi::p2::bindings::io,
        "wasi:logging": crate::internal::wasm::logging::bindings,
        "isola:script/host.value-iterator": host::ValueIterator,
        "isola:script/host.future-hostcall": host::FutureHostcall,
    },
});

use std::{future::Future, sync::Arc};

use bytes::Bytes;
pub use exports::isola::script::guest;
use wasmtime::component::{HasData, Linker};
use wasmtime_wasi::ResourceTable;

pub mod host;

pub enum EmitValue {
    Continuation(Bytes),
    PartialResult(Bytes),
    End(Bytes),
}

pub trait HostView: Send {
    type Host: crate::Host;

    fn table(&mut self) -> &mut ResourceTable;

    fn host(&mut self) -> &Arc<Self::Host>;

    fn emit(&mut self, data: EmitValue) -> impl Future<Output = wasmtime::Result<()>> + Send;
}

impl<T: ?Sized + HostView> HostView for &mut T {
    type Host = T::Host;

    fn table(&mut self) -> &mut ResourceTable {
        T::table(self)
    }

    fn host(&mut self) -> &Arc<Self::Host> {
        T::host(self)
    }

    async fn emit(&mut self, data: EmitValue) -> wasmtime::Result<()> {
        T::emit(self, data).await
    }
}

struct HostImpl<T>(T);

pub fn add_to_linker<T: HostView>(l: &mut Linker<T>) -> anyhow::Result<()> {
    struct Host<T>(T);
    impl<T: 'static> HasData for Host<T> {
        type Data<'a> = HostImpl<&'a mut T>;
    }
    self::isola::script::host::add_to_linker::<_, Host<T>>(l, |t| HostImpl(t))?;
    Ok(())
}
