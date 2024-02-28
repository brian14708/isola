use std::pin::Pin;

use tokio_stream::{Stream, StreamExt};
use wasmtime::component::ResourceTable;

use super::bindgen::promptkit::script::types::{self, Argument};

pub trait HostTypesCtx: Send {
    fn table(&mut self) -> &mut ResourceTable;
}

impl<I> types::Host for I where I: HostTypesCtx + Sync {}

pub struct ArgumentIterator {
    stream: Pin<Box<dyn Stream<Item = Argument> + Send>>,
}

impl ArgumentIterator {
    pub fn new(stream: Pin<Box<dyn Stream<Item = Argument> + Send>>) -> Self {
        Self { stream }
    }
}

#[async_trait::async_trait]
impl<I> types::HostArgumentIterator for I
where
    I: HostTypesCtx + Sync,
{
    async fn read(
        &mut self,
        resource: wasmtime::component::Resource<ArgumentIterator>,
    ) -> wasmtime::Result<Option<Argument>> {
        let response = self.table().get_mut(&resource)?;
        Ok(response.stream.next().await)
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<ArgumentIterator>,
    ) -> wasmtime::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}
