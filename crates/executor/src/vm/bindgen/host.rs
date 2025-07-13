use std::pin::Pin;

use futures_util::{FutureExt, StreamExt};
use tokio_stream::Stream;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::{DynPollable, Pollable, bindings::io::streams::StreamError};

pub use super::promptkit::script::host::Value;
use super::{
    HostView,
    promptkit::script::host::{Host, HostValueIterator},
};

pub struct ValueIterator {
    pub(crate) stream: Pin<Box<dyn Stream<Item = Value> + Send>>,
    pub(crate) peek: Option<Result<Value, StreamError>>,
}

impl ValueIterator {
    pub fn new(stream: impl Stream<Item = Value> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            peek: None,
        }
    }

    pub async fn next(&mut self) -> Result<Value, StreamError> {
        match self.peek.take() {
            Some(v) => v,
            None => match self.stream.next().await {
                Some(v) => Ok(v),
                None => Err(StreamError::Closed),
            },
        }
    }

    pub fn try_next(&mut self) -> Option<Result<Value, StreamError>> {
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
impl Pollable for ValueIterator {
    async fn ready(&mut self) {
        if self.peek.is_none() {
            self.peek = match self.stream.next().await {
                Some(v) => Some(Ok(v)),
                None => Some(Err(StreamError::Closed)),
            }
        }
    }
}

impl<T: HostView> Host for super::HostImpl<T> {
    async fn emit(&mut self, data: Vec<u8>) -> wasmtime::Result<()> {
        self.0.emit(data).await
    }
}

impl<T: HostView> HostValueIterator for super::HostImpl<T> {
    async fn read(
        &mut self,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Option<Result<Value, StreamError>>> {
        Ok(self.0.table().get_mut(&resource)?.try_next())
    }

    async fn blocking_read(
        &mut self,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Result<Value, StreamError>> {
        let response = self.0.table().get_mut(&resource)?;
        Ok(response.next().await)
    }

    async fn subscribe(
        &mut self,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), resource)
    }

    async fn drop(&mut self, rep: Resource<ValueIterator>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}
