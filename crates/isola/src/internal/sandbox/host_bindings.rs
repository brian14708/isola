use std::{pin::Pin, sync::Arc};

use futures::StreamExt;
use tokio_stream::Stream;
use tracing::Instrument;
use wasmtime::component::{Accessor, Resource};

use super::{
    EmitValue, HostImpl, HostView, LinkerHost,
    isola::script::host::{
        EmitType, Host, HostValueIterator, HostValueIteratorWithStore, HostWithStore,
    },
};
use crate::{host::Host as _, value::Value};

pub struct ValueIterator {
    stream: Pin<Box<dyn Stream<Item = Value> + Send>>,
}

impl ValueIterator {
    #[must_use]
    pub fn new(stream: Pin<Box<dyn Stream<Item = Value> + Send>>) -> Self {
        Self { stream }
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
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "WIT-generated host traits are clearer as async methods even when some return immediately"
)]
impl<T: HostView> HostValueIterator for HostImpl<T> {
    async fn drop(&mut self, rep: Resource<ValueIterator>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

impl<T: HostView + 'static> HostValueIteratorWithStore<T> for LinkerHost<T> {
    async fn read(
        accessor: &Accessor<T, Self>,
        resource: Resource<ValueIterator>,
    ) -> wasmtime::Result<Option<Vec<u8>>> {
        // Take the stream out (leaving an inert placeholder) so we can await
        // without holding the store across the await point. The resource stays
        // resident in the table, so its rep is preserved by construction rather
        // than relying on ResourceTable slot-reuse ordering.
        let mut stream = accessor.with(|mut access| -> wasmtime::Result<_> {
            let iter = access.get().0.table().get_mut(&resource)?;
            Ok(std::mem::replace(
                &mut iter.stream,
                Box::pin(futures::stream::empty()),
            ))
        })?;
        let value = stream.next().await.map(|v| v.into_cbor().into());
        accessor.with(|mut access| -> wasmtime::Result<()> {
            access.get().0.table().get_mut(&resource)?.stream = stream;
            Ok(())
        })?;
        Ok(value)
    }
}

impl<T: HostView + 'static> HostWithStore<T> for LinkerHost<T> {
    async fn hostcall(
        accessor: &Accessor<T, Self>,
        call_type: String,
        payload: Vec<u8>,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        let host = accessor.with(|mut access| Arc::clone(access.get().0.host()));
        Ok(wasmtime_wasi::runtime::spawn(
            async move {
                let payload = Value::from_cbor(payload);
                host.hostcall(&call_type, payload)
                    .await
                    .map(|v| v.into_cbor().into())
                    .map_err(|e| e.to_string())
            }
            .in_current_span(),
        )
        .await)
    }
}
