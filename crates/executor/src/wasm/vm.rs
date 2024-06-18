use tracing::event;
use wasmtime_wasi::{bindings::io::streams::StreamError, Pollable, ResourceTable};

use self::bindings::host::HostArgumentIterator;
use bindings::host::{Argument, LogLevel};

wasmtime::component::bindgen!({
    path: "../../apis/wit",
    interfaces: "import promptkit:vm/host;",
    async: true,
    trappable_imports: true,
    with: {
        "wasi": wasmtime_wasi::bindings,
        "promptkit:vm/host/argument-iterator": types::ArgumentIterator,
    },
});

pub use promptkit::vm as bindings;

pub mod types {
    use std::pin::Pin;

    use futures_util::{FutureExt, StreamExt};
    use tokio_stream::Stream;
    use wasmtime_wasi::{bindings::io::streams::StreamError, Subscribe};

    pub use super::bindings::host::Argument;

    pub struct ArgumentIterator {
        pub(crate) stream: Pin<Box<dyn Stream<Item = Argument> + Send>>,
        pub(crate) peek: Option<Result<Argument, StreamError>>,
    }

    impl ArgumentIterator {
        pub fn new(stream: Pin<Box<dyn Stream<Item = Argument> + Send>>) -> Self {
            Self { stream, peek: None }
        }

        pub async fn next(&mut self) -> Result<Argument, StreamError> {
            match self.peek.take() {
                Some(v) => v,
                None => match self.stream.next().await {
                    Some(v) => Ok(v),
                    None => Err(StreamError::Closed),
                },
            }
        }

        pub fn try_next(&mut self) -> Option<Result<Argument, StreamError>> {
            match self.peek.take() {
                Some(v) => Some(v),
                None => match self.stream.next().now_or_never() {
                    None => None,
                    Some(None) => Some(Err(StreamError::Closed)),
                    Some(Some(v)) => Some(Ok(v)),
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl Subscribe for ArgumentIterator {
        async fn ready(&mut self) {
            if self.peek.is_none() {
                self.peek = match self.stream.next().await {
                    Some(v) => Some(Ok(v)),
                    None => Some(Err(StreamError::Closed)),
                }
            }
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
    ) -> wasmtime::Result<Option<Result<Argument, StreamError>>> {
        Ok(self.table().get_mut(&resource)?.try_next())
    }

    async fn blocking_read(
        &mut self,
        resource: wasmtime::component::Resource<types::ArgumentIterator>,
    ) -> wasmtime::Result<Result<Argument, StreamError>> {
        let response = self.table().get_mut(&resource)?;
        Ok(response.next().await)
    }

    async fn subscribe(
        &mut self,
        resource: wasmtime::component::Resource<types::ArgumentIterator>,
    ) -> wasmtime::Result<wasmtime::component::Resource<Pollable>> {
        wasmtime_wasi::subscribe(self.table(), resource)
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<types::ArgumentIterator>,
    ) -> wasmtime::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}
