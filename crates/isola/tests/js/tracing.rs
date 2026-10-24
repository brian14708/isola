use std::sync::Arc;

use anyhow::Result;
use isola::{
    host::{
        BoxError, Host, HttpRequestStream, HttpResponse, LogContext, LogLevel, OutputSink,
        OutputTarget,
    },
    sandbox::SandboxOptions,
    value::Value,
};
use parking_lot::Mutex;
use tracing::{Instrument as _, instrument::WithSubscriber as _, span::Id};
use tracing_subscriber::registry::LookupSpan as _;

use super::common::build_module;

#[derive(Clone)]
struct SpanRecorder {
    expected: Id,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl SpanRecorder {
    async fn record(&self, operation: &'static str) {
        tokio::task::yield_now().await;
        let current = tracing::Span::current().id();
        tracing::dispatcher::get_default(|dispatcher| {
            let registry = dispatcher
                .downcast_ref::<tracing_subscriber::Registry>()
                .unwrap();
            let span = registry
                .span(current.as_ref().expect("missing callback span"))
                .unwrap();
            assert!(
                span.scope().any(|span| span.id() == self.expected),
                "call span lost in {operation}"
            );
        });
        self.calls.lock().push(operation);
    }
}

impl Host for SpanRecorder {
    async fn hostcall(&self, _: &str, value: Value) -> Result<Value, BoxError> {
        self.record("hostcall").await;
        Ok(value)
    }

    async fn http_request_stream(&self, _: HttpRequestStream) -> Result<HttpResponse, BoxError> {
        self.record("http").await;
        Ok(http::Response::new(Box::pin(futures::stream::empty())))
    }
}

impl OutputSink for SpanRecorder {
    async fn on_item(&self, _: Value) -> Result<(), BoxError> {
        self.record("item").await;
        Ok(())
    }

    async fn on_complete(&self, _: Option<Value>) -> Result<(), BoxError> {
        self.record("complete").await;
        Ok(())
    }

    async fn on_log(
        &self,
        level: LogLevel,
        _: LogContext<'_>,
        message: &str,
    ) -> Result<(), BoxError> {
        assert_eq!(level, LogLevel::Info);
        let value: serde_json::Value = serde_json::from_str(message)?;
        assert_eq!(value["message"], "line\nC:\\temp \"\t\0");
        self.record("log").await;
        Ok(())
    }
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "integration tests run in release mode")]
async fn integration_js_tracing_preserves_local_subscriber_across_callbacks() -> Result<()> {
    let Some(module) = build_module().await? else {
        return Ok(());
    };
    // A local subscriber must follow callbacks onto spawned tasks.
    let dispatcher = tracing::Dispatch::new(tracing_subscriber::registry());
    let span = tracing::dispatcher::with_default(&dispatcher, || tracing::info_span!("guest_call"));
    let recorder = SpanRecorder {
        expected: span.id().unwrap(),
        calls: Arc::default(),
    };
    let mut sandbox = module
        .instantiate(recorder.clone(), SandboxOptions::default())
        .await?;
    sandbox
        .eval_script(
            r#"async function main() {
            await hostcall('echo', 42);
            await (await fetch('http://tracing.test/')).text();
            console.log('line\nC:\\temp "\t\u0000');
            return 42;
        }"#,
            OutputTarget::discard(),
        )
        .await?;

    sandbox
        .call_with_sink(
            "main",
            [],
            OutputTarget::asynchronous(Arc::new(recorder.clone())),
        )
        .instrument(span)
        .with_subscriber(dispatcher)
        .await?;
    assert_eq!(
        *recorder.calls.lock(),
        ["hostcall", "http", "log", "complete"]
    );
    Ok(())
}
