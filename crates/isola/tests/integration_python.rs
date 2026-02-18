use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use bytes::Bytes;
use isola::module::ArgValue;
use isola::{
    Arg, BoxError, CacheConfig, CallOptions, CompileConfig, Host, HttpRequest, HttpResponse,
    Module, ModuleBuilder, OutputSink, Sandbox, TRACE_TARGET_SCRIPT, WebsocketRequest,
    WebsocketResponse,
};
use isola_trace::collect::{CollectLayer, CollectSpanExt, Collector, EventRecord, SpanRecord};
use promptkit_cbor::{from_cbor, json_to_cbor};
use tracing::{info_span, level_filters::LevelFilter};
use tracing_subscriber::{Registry, layer::SubscriberExt};

#[derive(Clone, Default)]
struct TestHost;

#[async_trait]
impl Host for TestHost {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Bytes,
    ) -> std::result::Result<Bytes, BoxError> {
        match call_type {
            "echo" => Ok(payload),
            _ => Err(std::io::Error::other(format!("unsupported hostcall: {call_type}")).into()),
        }
    }

    async fn http_request(&self, _req: HttpRequest) -> std::result::Result<HttpResponse, BoxError> {
        Err(
            std::io::Error::other("http_request is not implemented in integration-bundle tests")
                .into(),
        )
    }

    async fn websocket_connect(
        &self,
        _req: WebsocketRequest,
    ) -> std::result::Result<WebsocketResponse, BoxError> {
        Err(std::io::Error::other(
            "websocket_connect is not implemented in integration-bundle tests",
        )
        .into())
    }
}

#[derive(Debug, Default, Clone)]
struct SinkState {
    partial: Vec<Bytes>,
    end: Vec<Bytes>,
}

struct CollectSink {
    state: Arc<Mutex<SinkState>>,
}

impl CollectSink {
    fn new(state: Arc<Mutex<SinkState>>) -> Self {
        Self { state }
    }
}

#[derive(Clone, Default)]
struct TraceCollector {
    events: Arc<Mutex<Vec<EventRecord>>>,
}

impl TraceCollector {
    fn events(&self) -> Vec<EventRecord> {
        self.events
            .lock()
            .expect("trace collector event lock poisoned")
            .clone()
    }
}

impl Collector for TraceCollector {
    fn on_span_start(&self, _span: SpanRecord) {}

    fn on_span_end(&self, _span: SpanRecord) {}

    fn on_event(&self, event: EventRecord) {
        self.events
            .lock()
            .expect("trace collector event lock poisoned")
            .push(event);
    }
}

#[async_trait]
impl OutputSink for CollectSink {
    async fn on_partial(&mut self, cbor: Bytes) -> std::result::Result<(), BoxError> {
        self.state
            .lock()
            .expect("sink state mutex poisoned")
            .partial
            .push(cbor);
        Ok(())
    }

    async fn on_end(&mut self, cbor: Bytes) -> std::result::Result<(), BoxError> {
        self.state
            .lock()
            .expect("sink state mutex poisoned")
            .end
            .push(cbor);
        Ok(())
    }
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("failed to resolve workspace root from CARGO_MANIFEST_DIR")
}

fn bundle_path(root: &Path) -> PathBuf {
    root.join("target").join("promptkit_python.wasm")
}

fn resolve_lib_dir(root: &Path) -> PathBuf {
    env::var_os("WASI_PYTHON_RUNTIME").map_or_else(
        || {
            root.join("target")
                .join("wasm32-wasip1")
                .join("wasi-deps")
                .join("usr")
                .join("local")
                .join("lib")
        },
        |p| PathBuf::from(p).join("lib"),
    )
}

fn cache_dir(root: &Path) -> PathBuf {
    root.join("target").join("cache")
}

fn is_cache_warm(cache: &Path) -> Result<bool> {
    if !cache.is_dir() {
        return Ok(false);
    }

    for entry in
        std::fs::read_dir(cache).with_context(|| format!("failed to read {}", cache.display()))?
    {
        let entry = entry.with_context(|| format!("failed to iterate {}", cache.display()))?;
        if entry.path().extension().is_some_and(|ext| ext == "cwasm") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn assert_prereqs() -> Result<(PathBuf, PathBuf)> {
    let root = workspace_root()?;
    let wasm = bundle_path(&root);
    let lib_dir = resolve_lib_dir(&root);
    let cache = cache_dir(&root);

    if !wasm.is_file() {
        bail!(
            "missing integration wasm bundle at '{}'. Build it with `cargo xtask build-all`.",
            wasm.display()
        );
    }

    if !lib_dir.is_dir() {
        bail!(
            "missing WASI runtime libraries at '{}'. Run in the dev shell or set \
             WASI_PYTHON_RUNTIME, then build with `cargo xtask build-all`.",
            lib_dir.display()
        );
    }

    if !is_cache_warm(&cache)? {
        bail!(
            "cold isola cache at '{}'. To keep this suite under 10s, warm the cache first with \
             `just integration-c` (or another real runtime run) and retry.",
            cache.display()
        );
    }

    Ok((wasm, lib_dir))
}

async fn build_module() -> Result<Module<TestHost>> {
    let (wasm, lib_dir) = assert_prereqs()?;

    let module = ModuleBuilder::new()
        .compile_config(CompileConfig {
            cache: CacheConfig::Default,
            ..CompileConfig::default()
        })
        .lib_dir(lib_dir)
        .build(&wasm)
        .await
        .context("failed to build module from integration wasm bundle")?;

    Ok(module)
}

async fn call_collect(
    sandbox: &mut Sandbox<TestHost>,
    function: &str,
    args: Vec<Arg>,
    timeout: Duration,
) -> std::result::Result<SinkState, isola::Error> {
    let state = Arc::new(Mutex::new(SinkState::default()));
    let sink = CollectSink::new(Arc::clone(&state));
    sandbox
        .call(
            function,
            args,
            sink,
            CallOptions::default().timeout(timeout),
        )
        .await?;
    Ok(state.lock().expect("sink state mutex poisoned").clone())
}

fn cbor_arg(name: Option<&str>, json: &str) -> Result<Arg> {
    let value =
        json_to_cbor(json).with_context(|| format!("failed to convert json to cbor: {json}"))?;
    Ok(Arg {
        name: name.map(str::to_owned),
        value: ArgValue::Cbor(value),
    })
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_eval_and_call_roundtrip() -> Result<()> {
    let collector = TraceCollector::default();
    let subscriber = Registry::default().with(CollectLayer::default());
    let _guard = tracing::subscriber::set_default(subscriber);

    let root = info_span!("integration_python_eval_and_call_roundtrip");
    root.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector.clone())
        .ok_or_else(|| anyhow::anyhow!("failed to install trace collector"))?;
    let _root = root.enter();

    let module = build_module().await?;
    let mut sandbox = module
        .instantiate(None, TestHost)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\tprint('trace-print')\n\treturn 42")
        .await
        .context("failed to evaluate script")?;

    let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed to call function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: i64 = from_cbor(state.end[0].as_ref()).context("failed to decode end output")?;
    assert_eq!(value, 42);

    let events = collector.events();
    let has_print = events.iter().any(|e| {
        e.name == "log"
            && e.properties
                .iter()
                .any(|(k, v)| *k == "log.context" && v == "stdout")
            && e.properties
                .iter()
                .any(|(k, v)| *k == "log.output" && v.contains("trace-print"))
    });
    assert!(
        has_print,
        "expected trace event for print output, events: {events:?}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_streaming_output() -> Result<()> {
    let module = build_module().await?;
    let mut sandbox = module
        .instantiate(None, TestHost)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\tfor i in range(3):\n\t\tyield i")
        .await
        .context("failed to evaluate streaming script")?;

    let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed to call streaming function")?;

    assert_eq!(state.partial.len(), 3, "expected three partial outputs");
    let mut values = Vec::with_capacity(state.partial.len());
    for item in &state.partial {
        values.push(from_cbor::<i64>(item.as_ref()).context("failed to decode partial output")?);
    }
    assert_eq!(values, vec![0, 1, 2]);

    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    if !state.end[0].is_empty() {
        let end_value: Option<i64> =
            from_cbor(state.end[0].as_ref()).context("failed to decode end output")?;
        assert_eq!(end_value, None, "expected empty end output or null value");
    }

    Ok(())
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_argument_cbor_path() -> Result<()> {
    let module = build_module().await?;
    let mut sandbox = module
        .instantiate(None, TestHost)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main(i, s):\n\treturn (i + 1, s.upper())")
        .await
        .context("failed to evaluate argument script")?;

    let args = vec![cbor_arg(None, "41")?, cbor_arg(Some("s"), "\"hello\"")?];
    let state = call_collect(&mut sandbox, "main", args, Duration::from_secs(2))
        .await
        .context("failed to call argument function")?;

    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");
    let value: (i64, String) =
        from_cbor(state.end[0].as_ref()).context("failed to decode argument result")?;
    assert_eq!(value, (42, "HELLO".to_string()));
    Ok(())
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_reinstantiate_smoke() -> Result<()> {
    let module = build_module().await?;

    for expected in [7_i64, 11_i64] {
        let mut sandbox = module
            .instantiate(None, TestHost)
            .await
            .context("failed to instantiate sandbox")?;

        sandbox
            .eval_script(format!("def main():\n\treturn {expected}"))
            .await
            .context("failed to evaluate script")?;

        let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
            .await
            .context("failed to call function")?;
        assert_eq!(state.end.len(), 1, "expected exactly one end output");
        let value: i64 =
            from_cbor(state.end[0].as_ref()).context("failed to decode roundtrip output")?;
        assert_eq!(value, expected);
    }

    Ok(())
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_async_hostcall_echo() -> Result<()> {
    let module = build_module().await?;
    let mut sandbox = module
        .instantiate(None, TestHost)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "import promptkit.asyncio as pk_async\n\
             async def main():\n\
             \treturn await pk_async.hostcall(\"echo\", [1, 2, 3])",
        )
        .await
        .context("failed to evaluate async hostcall script")?;

    let state = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed to call async hostcall function")?;
    assert!(state.partial.is_empty(), "expected no partial outputs");
    assert_eq!(state.end.len(), 1, "expected exactly one end output");

    let value: Vec<i64> =
        from_cbor(state.end[0].as_ref()).context("failed to decode hostcall response")?;
    assert_eq!(value, vec![1, 2, 3]);

    Ok(())
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_guest_exception_surface() -> Result<()> {
    let module = build_module().await?;
    let mut sandbox = module
        .instantiate(None, TestHost)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script("def main():\n\traise RuntimeError(\"boom\")")
        .await
        .context("failed to evaluate exception script")?;

    let err = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .expect_err("expected exception from guest function");
    let isola::Error::Guest { message, .. } = err else {
        panic!("expected guest error, got {err:?}");
    };
    assert!(
        message.contains("boom"),
        "unexpected error message: {message}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires integration wasm bundle"]
async fn integration_python_state_persists_within_sandbox() -> Result<()> {
    let module = build_module().await?;
    let mut sandbox = module
        .instantiate(None, TestHost)
        .await
        .context("failed to instantiate sandbox")?;

    sandbox
        .eval_script(
            "counter = 0\n\
             def main():\n\
             \tglobal counter\n\
             \tcounter += 1\n\
             \treturn counter",
        )
        .await
        .context("failed to evaluate stateful script")?;

    let first = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed first stateful call")?;
    let second = call_collect(&mut sandbox, "main", vec![], Duration::from_secs(2))
        .await
        .context("failed second stateful call")?;

    assert_eq!(first.end.len(), 1, "expected exactly one first end output");
    assert_eq!(
        second.end.len(),
        1,
        "expected exactly one second end output"
    );
    let first_v: i64 = from_cbor(first.end[0].as_ref()).context("failed to decode first value")?;
    let second_v: i64 =
        from_cbor(second.end[0].as_ref()).context("failed to decode second value")?;
    assert_eq!(first_v, 1);
    assert_eq!(second_v, 2);

    Ok(())
}
