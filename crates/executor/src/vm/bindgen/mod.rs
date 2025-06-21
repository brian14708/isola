wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../apis/wit",
    async: true,
    trappable_imports: true,
    with: {
        "wasi:io": wasmtime_wasi::p2::bindings::io,
        "wasi:logging": crate::wasm::logging::bindings,
        "promptkit:script/host/value-iterator": host::ValueIterator,
        "promptkit:script/outgoing-rpc/connection": outgoing_rpc::Connection,
        "promptkit:script/outgoing-rpc/future-connection": outgoing_rpc::FutureConnection,
        "promptkit:script/outgoing-rpc/connect-request": outgoing_rpc::ConnectRequest,
        "promptkit:script/outgoing-rpc/payload": outgoing_rpc::Payload,
        "promptkit:script/outgoing-rpc/request-stream": outgoing_rpc::RequestStream,
        "promptkit:script/outgoing-rpc/response-stream": outgoing_rpc::ResponseStream,
    },
});

use std::future::Future;

pub use exports::promptkit::script::guest;
use wasmtime::component::{HasData, Linker};
use wasmtime_wasi::p2::IoView;

pub trait HostView: IoView + Send {
    type Env: crate::Env + Send;
    fn env(&mut self) -> &mut Self::Env;
    fn emit(&mut self, data: Vec<u8>) -> impl Future<Output = wasmtime::Result<()>> + Send;
}

impl<T: ?Sized + HostView> HostView for &mut T {
    type Env = T::Env;
    fn env(&mut self) -> &mut Self::Env {
        T::env(self)
    }

    fn emit(&mut self, data: Vec<u8>) -> impl Future<Output = wasmtime::Result<()>> + Send {
        T::emit(self, data)
    }
}

struct HostImpl<T>(T);

pub mod host;
pub mod outgoing_rpc;

pub fn add_to_linker<T: HostView>(l: &mut Linker<T>) -> anyhow::Result<()> {
    struct Host<T>(T);
    impl<T: 'static> HasData for Host<T> {
        type Data<'a> = HostImpl<&'a mut T>;
    }
    promptkit::script::host::add_to_linker::<_, Host<T>>(l, |t| HostImpl(t))?;
    promptkit::script::outgoing_rpc::add_to_linker::<_, Host<T>>(l, |t| HostImpl(t))?;
    Ok(())
}
