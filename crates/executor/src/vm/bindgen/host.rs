use std::pin::Pin;

use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use tokio_stream::Stream;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::{DynPollable, Pollable, bindings::io::streams::StreamError};

use crate::vm::bindgen::EmitValue;

use super::{
    HostView,
    promptkit::script::host::{Host, HostValueIterator},
};

pub struct ValueIterator {
    stream: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    peek: Option<Result<Bytes, StreamError>>,
}

impl ValueIterator {
    #[must_use]
    pub fn new(stream: Pin<Box<dyn Stream<Item = Bytes> + Send>>) -> Self {
        Self { stream, peek: None }
    }

    async fn next(&mut self) -> Result<Vec<u8>, StreamError> {
        match self.peek.take() {
            Some(Ok(v)) => Ok(v.into()),
            Some(Err(e)) => Err(e),
            None => match self.stream.next().await {
                Some(v) => Ok(v.into()),
                None => Err(StreamError::Closed),
            },
        }
    }

    fn try_next(&mut self) -> Option<Result<Vec<u8>, StreamError>> {
        match self.peek.take() {
            Some(Ok(v)) => Some(Ok(v.into())),
            Some(Err(e)) => Some(Err(e)),
            None => match self.stream.next().now_or_never() {
                None => None,
                Some(None) => Some(Err(StreamError::Closed)),
                Some(Some(v)) => Some(Ok(v.into())),
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
    async fn blocking_emit(
        &mut self,
        emit_type: super::promptkit::script::host::EmitType,
        cbor: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let emit_value = match emit_type {
            super::promptkit::script::host::EmitType::Continuation => {
                EmitValue::Continuation(cbor.into())
            }
            super::promptkit::script::host::EmitType::End => EmitValue::End(cbor.into()),
            super::promptkit::script::host::EmitType::PartialResult => {
                EmitValue::PartialResult(cbor.into())
            }
        };
        self.0.emit(emit_value).await
    }
}

impl<T: HostView> HostValueIterator for super::HostImpl<T> {
    async fn read(
        &mut self,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Option<Result<Vec<u8>, StreamError>>> {
        Ok(self.0.table().get_mut(&resource)?.try_next())
    }

    async fn blocking_read(
        &mut self,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Result<Vec<u8>, StreamError>> {
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
