use std::pin::Pin;

use bytes::Bytes;
use futures::{FutureExt, StreamExt};
use tokio_stream::Stream;
use tracing::Instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::{
    p2::{DynPollable, IoError, Pollable, bindings::io::streams::StreamError},
    runtime::AbortOnDropJoinHandle,
};

use super::EmitValue;
use super::{
    HostView,
    isola::script::host::{Host, HostFutureHostcall, HostValueIterator},
};
use crate::Host as _;

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
            None => (self.stream.next().await).map_or(Err(StreamError::Closed), |v| Ok(v.into())),
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

impl<T: HostView> Host for super::HostImpl<T> {
    async fn blocking_emit(
        &mut self,
        emit_type: super::isola::script::host::EmitType,
        cbor: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let emit_value = match emit_type {
            super::isola::script::host::EmitType::Continuation => {
                EmitValue::Continuation(cbor.into())
            }
            super::isola::script::host::EmitType::End => EmitValue::End(cbor.into()),
            super::isola::script::host::EmitType::PartialResult => {
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
        let host = self.0.host().clone();

        let s = wasmtime_wasi::runtime::spawn(
            async move {
                let payload: Bytes = payload.into();
                host.hostcall(&call_type, payload)
                    .await
                    .map(|b| b.to_vec())
                    .map_err(|e| anyhow::Error::msg(e.to_string()))
            }
            .in_current_span(),
        );
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

#[cfg(all(test, feature = "trace"))]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::trace::collect::{CollectLayer, CollectSpanExt, Collector, EventRecord, SpanRecord};
    use bytes::Bytes;
    use tracing::{event, info_span, level_filters::LevelFilter};
    use tracing_subscriber::{Registry, layer::SubscriberExt};
    use wasmtime_wasi::ResourceTable;

    use super::{Host as WitHost, HostFutureHostcall as WitHostFutureHostcall, *};
    use crate::{BoxError, Host as EmbedderHost, TRACE_TARGET_SCRIPT};

    #[derive(Clone)]
    struct VecCollector(Arc<Mutex<(Vec<SpanRecord>, Vec<EventRecord>)>>);

    impl Collector for VecCollector {
        fn on_span_start(&self, v: SpanRecord) {
            self.0.lock().expect("lock poisoned").0.push(v);
        }

        fn on_span_end(&self, _v: SpanRecord) {}

        fn on_event(&self, v: EventRecord) {
            self.0.lock().expect("lock poisoned").1.push(v);
        }
    }

    #[derive(Clone)]
    struct TestHost;

    #[async_trait::async_trait]
    impl EmbedderHost for TestHost {
        async fn hostcall(
            &self,
            _call_type: &str,
            _payload: Bytes,
        ) -> core::result::Result<Bytes, BoxError> {
            event!(
                name: "hostcall",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::INFO,
                test_marker = "hostcall",
            );
            Ok(Bytes::from_static(b"ok"))
        }

        async fn http_request(
            &self,
            _req: crate::HttpRequest,
        ) -> core::result::Result<crate::HttpResponse, BoxError> {
            Err(std::io::Error::other("unsupported").into())
        }

        async fn websocket_connect(
            &self,
            _req: crate::WebsocketRequest,
        ) -> core::result::Result<crate::WebsocketResponse, BoxError> {
            Err(std::io::Error::other("unsupported").into())
        }
    }

    struct TestView {
        table: ResourceTable,
        host: TestHost,
        policy: crate::net::AllowAllPolicy,
    }

    impl super::super::HostView for TestView {
        type Host = TestHost;

        fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }

        fn host(&mut self) -> &mut Self::Host {
            &mut self.host
        }

        fn network_policy(&self) -> &dyn crate::NetworkPolicy {
            &self.policy
        }

        fn emit(
            &mut self,
            _data: super::super::EmitValue,
        ) -> impl core::future::Future<Output = wasmtime::Result<()>> + Send {
            async { Ok(()) }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hostcall_spawn_preserves_current_span() {
        let collector = VecCollector(Arc::new(Mutex::new((Vec::new(), Vec::new()))));
        let subscriber = Registry::default().with(CollectLayer::default());
        let _guard = tracing::subscriber::set_default(subscriber);

        let root = info_span!("root");
        root.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::INFO, collector.clone())
            .expect("collect_into");

        let _root = root.enter();
        let call_span = info_span!(target: TRACE_TARGET_SCRIPT, "call_span");
        let _call = call_span.enter();

        let mut view = TestView {
            table: ResourceTable::new(),
            host: TestHost,
            policy: crate::net::AllowAllPolicy,
        };

        let future = {
            let mut host_impl = super::super::HostImpl(&mut view);
            WitHost::hostcall(&mut host_impl, "x".to_string(), Vec::new())
                .await
                .expect("hostcall")
        };

        view.table
            .get_mut(&future)
            .expect("future stored")
            .ready()
            .await;

        let ready = {
            let mut host_impl = super::super::HostImpl(&mut view);
            WitHostFutureHostcall::get(&mut host_impl, future)
                .await
                .expect("get")
                .expect("ready")
        };

        let data = match ready {
            Ok(Ok(data)) => data,
            other => panic!("unexpected hostcall result: {other:?}"),
        };
        assert_eq!(data, b"ok".to_vec());

        let (spans, events) = collector.0.lock().expect("lock poisoned").clone();
        assert_eq!(spans.len(), 1);
        assert_eq!(events.len(), 1);

        assert_eq!(spans[0].name, "call_span");
        assert_eq!(events[0].name, "hostcall");
        assert!(
            events[0]
                .properties
                .iter()
                .any(|(k, v)| *k == "test_marker" && v == "hostcall")
        );
        assert_eq!(events[0].parent_span_id, spans[0].span_id);
    }
}
