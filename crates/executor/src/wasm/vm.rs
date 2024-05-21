use tokio_stream::StreamExt;
use tracing::event;
use wasmtime_wasi::ResourceTable;

use self::bindings::host::HostArgumentIterator;
use bindings::host::{Argument, LogLevel};

wasmtime::component::bindgen!({
    path: "../../apis/wit",
    interfaces: "import promptkit:vm/host;",
    async: true,
    trappable_imports: true,
    with: {
        "promptkit:vm/host/argument-iterator": types::ArgumentIterator,
    },
});

pub use promptkit::vm as bindings;

pub mod types {
    use std::pin::Pin;

    use futures_util::Stream;

    pub use super::bindings::host::Argument;

    pub struct ArgumentIterator {
        pub(crate) stream: Pin<Box<dyn Stream<Item = Argument> + Send>>,
    }

    impl ArgumentIterator {
        pub fn new(stream: Pin<Box<dyn Stream<Item = Argument> + Send>>) -> Self {
            Self { stream }
        }
    }
}

#[async_trait::async_trait]
pub trait VmView: Send {
    fn table(&mut self) -> &mut ResourceTable;

    async fn emit(&mut self, data: Vec<u8>) -> wasmtime::Result<()>;
}

pub fn add_to_linker<T: VmView>(
    linker: &mut wasmtime::component::Linker<T>,
) -> wasmtime::Result<()> {
    fn type_annotate<T, F>(val: F) -> F
    where
        F: Fn(&mut T) -> &mut dyn VmView,
    {
        val
    }
    let closure = type_annotate::<T, _>(|t| t);
    bindings::host::add_to_linker_get_host(linker, closure)
}

#[async_trait::async_trait]
impl bindings::host::Host for dyn VmView + '_ {
    async fn emit(&mut self, data: Vec<u8>) -> wasmtime::Result<()> {
        VmView::emit(self, data).await?;
        Ok(())
    }

    async fn emit_log(&mut self, log_level: LogLevel, data: String) -> wasmtime::Result<()> {
        match log_level {
            LogLevel::Debug => event!(
                target: "promptkit::debug",
                tracing::Level::DEBUG,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Info => event!(
                target: "promptkit::info",
                tracing::Level::INFO,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Warn => event!(
                target: "promptkit::warn",
                tracing::Level::WARN,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Error => event!(
                target: "promptkit::error",
                tracing::Level::ERROR,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
        };
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostArgumentIterator for dyn VmView + '_ {
    async fn read(
        &mut self,
        resource: wasmtime::component::Resource<types::ArgumentIterator>,
    ) -> wasmtime::Result<Option<Argument>> {
        let response = self.table().get_mut(&resource)?;
        Ok(response.stream.next().await)
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<types::ArgumentIterator>,
    ) -> wasmtime::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}
