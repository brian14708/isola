use std::{pin::Pin, sync::Arc};

use futures::{FutureExt, StreamExt};
use tokio_stream::Stream;
use tracing::Instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::{
    p2::{DynPollable, IoError, Pollable, bindings::io::streams::StreamError},
    runtime::AbortOnDropJoinHandle,
};

use super::{
    EmitValue, HostImpl, HostView,
    isola::script::host::{EmitType, Host, HostFutureHostcall, HostValueIterator},
};
use crate::{host::Host as _, value::Value};

pub struct ValueIterator {
    stream: Pin<Box<dyn Stream<Item = Value> + Send>>,
    peek: Option<Result<Value, StreamError>>,
}

impl ValueIterator {
    #[must_use]
    pub fn new(stream: Pin<Box<dyn Stream<Item = Value> + Send>>) -> Self {
        Self { stream, peek: None }
    }

    async fn next(&mut self) -> Result<Vec<u8>, StreamError> {
        match self.peek.take() {
            Some(Ok(v)) => Ok(v.into_cbor().to_vec()),
            Some(Err(e)) => Err(e),
            None => (self.stream.next().await)
                .map_or(Err(StreamError::Closed), |v| Ok(v.into_cbor().to_vec())),
        }
    }

    fn try_next(&mut self) -> Option<Result<Vec<u8>, StreamError>> {
        match self.peek.take() {
            Some(Ok(v)) => Some(Ok(v.into_cbor().to_vec())),
            Some(Err(e)) => Some(Err(e)),
            None => match self.stream.next().now_or_never() {
                None => None,
                Some(None) => Some(Err(StreamError::Closed)),
                Some(Some(v)) => Some(Ok(v.into_cbor().to_vec())),
            },
        }
    }
}

#[async_trait::async_trait]
impl Pollable for ValueIterator {
    async fn ready(&mut self) {
        if self.peek.is_none() {
            self.peek = (self.stream.next().await)
                .map_or_else(|| Some(Err(StreamError::Closed)), |v| Some(Ok(v)));
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

impl<T: HostView> Host for HostImpl<T> {
    async fn blocking_emit(&mut self, emit_type: EmitType, cbor: Vec<u8>) -> wasmtime::Result<()> {
        let emit_value = match emit_type {
            EmitType::Continuation => EmitValue::Continuation(cbor.into()),
            EmitType::End => EmitValue::End(cbor.into()),
            EmitType::PartialResult => EmitValue::PartialResult(cbor.into()),
        };
        self.0.emit(emit_value).await
    }

    async fn hostcall(
        &mut self,
        call_type: String,
        payload: Vec<u8>,
    ) -> wasmtime::Result<Resource<FutureHostcall>> {
        let host = Arc::clone(self.0.host());

        let s = wasmtime_wasi::runtime::spawn(
            async move {
                let payload = Value::from_cbor(payload);
                host.hostcall(&call_type, payload)
                    .await
                    .map(|v| v.into_cbor().to_vec())
                    .map_err(anyhow::Error::from_boxed)
            }
            .in_current_span(),
        );
        Ok(self.0.table().push(FutureHostcall::Pending(s))?)
    }
}

impl<T: HostView> HostValueIterator for HostImpl<T> {
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

impl<T: HostView> HostFutureHostcall for HostImpl<T> {
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
