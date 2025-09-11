use std::pin::Pin;

use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use tokio_stream::Stream;
use wasmtime::component::Resource;
use wasmtime_wasi::{
    p2::{DynPollable, IoError, Pollable, bindings::io::streams::StreamError},
    runtime::AbortOnDropJoinHandle,
};

use crate::{env::Env, vm::bindgen::EmitValue};

use super::{
    HostView,
    promptkit::script::host::{Host, HostFutureHostcall, HostValueIterator},
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

pub enum FutureHostcall {
    Pending(AbortOnDropJoinHandle<wasmtime::Result<Vec<u8>>>),
    Ready(wasmtime::Result<Vec<u8>>),
    Consumed,
}

#[async_trait::async_trait]
impl Pollable for FutureHostcall {
    async fn ready(&mut self) {
        if let Self::Pending(handle) = self {
            *self = Self::Ready(handle.await);
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

    async fn hostcall(
        &mut self,
        call_type: String,
        payload: Vec<u8>,
    ) -> wasmtime::Result<Resource<FutureHostcall>> {
        let env = self.0.env()?;

        let s = wasmtime_wasi::runtime::spawn(async move {
            env.hostcall(&call_type, &payload)
                .await
                .map_err(std::convert::Into::into)
        });
        Ok(self.0.table().push(FutureHostcall::Pending(s))?)
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

impl<T: HostView> HostFutureHostcall for super::HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<FutureHostcall>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn get(
        &mut self,
        self_: Resource<FutureHostcall>,
    ) -> wasmtime::Result<Option<Result<Result<Vec<u8>, Resource<IoError>>, ()>>> {
        let future = self.0.table().get_mut(&self_)?;
        match future {
            FutureHostcall::Ready(_) => match std::mem::replace(future, FutureHostcall::Consumed) {
                FutureHostcall::Ready(Ok(data)) => Ok(Some(Ok(Ok(data)))),
                FutureHostcall::Ready(Err(e)) => {
                    let error_resource = self.0.table().push(e)?;
                    Ok(Some(Ok(Err(error_resource))))
                }
                FutureHostcall::Pending(_) | FutureHostcall::Consumed => unreachable!(),
            },
            FutureHostcall::Pending(_) => Ok(None),
            FutureHostcall::Consumed => Ok(Some(Err(()))),
        }
    }

    async fn drop(&mut self, rep: Resource<FutureHostcall>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}
