use std::pin::Pin;

use futures_util::{FutureExt, StreamExt};
use tokio_stream::Stream;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::{DynPollable, Pollable, bindings::io::streams::StreamError};

use crate::vm::bindgen::EmitValue;

use super::{
    HostView,
    promptkit::script::host::{Host, HostValueIterator},
};

pub struct ValueIterator {
    pub(crate) stream: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
    pub(crate) peek: Option<Result<Vec<u8>, StreamError>>,
}

impl ValueIterator {
    pub fn new(stream: impl Stream<Item = Vec<u8>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            peek: None,
        }
    }

    pub async fn next(&mut self) -> Result<Vec<u8>, StreamError> {
        match self.peek.take() {
            Some(v) => v,
            None => match self.stream.next().await {
                Some(v) => Ok(v),
                None => Err(StreamError::Closed),
            },
        }
    }

    pub fn try_next(&mut self) -> Option<Result<Vec<u8>, StreamError>> {
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
    async fn blocking_emit(
        &mut self,
        emit_type: super::promptkit::script::host::EmitType,
        cbor: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let emit_value = match emit_type {
            super::promptkit::script::host::EmitType::Continuation => EmitValue::Continuation(cbor),
            super::promptkit::script::host::EmitType::End => EmitValue::End(cbor),
            super::promptkit::script::host::EmitType::PartialResult => {
                EmitValue::PartialResult(cbor)
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
