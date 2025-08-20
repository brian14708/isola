wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../specs/wit",
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
        "wasi:logging": crate::wasm::logging::bindings,
        "promptkit:script/host/value-iterator": host::ValueIterator,
        "promptkit:script/outgoing-websocket/connect-request": outgoing_websocket::ConnectRequest,
        "promptkit:script/outgoing-websocket/websocket-message": outgoing_websocket::WebsocketMessage,
        "promptkit:script/outgoing-websocket/read-stream": outgoing_websocket::ReadStream,
        "promptkit:script/outgoing-websocket/write-stream": outgoing_websocket::WriteStream,
        "promptkit:script/outgoing-websocket/websocket-connection": outgoing_websocket::WebsocketConnection,
        "promptkit:script/outgoing-websocket/future-websocket": outgoing_websocket::FutureWebsocket,
    },
});

use std::future::Future;

use bytes::Bytes;
pub use exports::promptkit::script::guest;
use wasmtime::component::{HasData, Linker};
use wasmtime_wasi::ResourceTable;

pub mod host;
pub mod outgoing_websocket;

pub enum EmitValue {
    Continuation(Bytes),
    PartialResult(Bytes),
    End(Bytes),
}

pub trait HostView: Send {
    type Env: crate::env::EnvHandle;
    fn table(&mut self) -> &mut ResourceTable;
    fn env(&mut self) -> wasmtime::Result<Self::Env>;
    fn emit(&mut self, data: EmitValue) -> impl Future<Output = wasmtime::Result<()>> + Send;
}

impl<T: ?Sized + HostView> HostView for &mut T {
    type Env = T::Env;

    fn env(&mut self) -> wasmtime::Result<Self::Env> {
        T::env(self)
    }

    fn table(&mut self) -> &mut ResourceTable {
        T::table(self)
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
    promptkit::script::host::add_to_linker::<_, Host<T>>(l, |t| HostImpl(t))?;
    promptkit::script::outgoing_websocket::add_to_linker::<_, Host<T>>(l, |t| HostImpl(t))?;
    Ok(())
}
