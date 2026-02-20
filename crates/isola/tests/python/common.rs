use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Once, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::Full;
use isola::cbor::json_to_cbor;
use isola::module::ArgValue;
use isola::trace::collect::{Collector, EventRecord, SpanRecord};
use isola::{
    Arg, BoxError, CacheConfig, CallOptions, CompileConfig, Host, HttpBodyStream, HttpRequest,
    HttpResponse, Module, ModuleBuilder, OutputSink, Sandbox,
    request::{Client, RequestOptions},
};

#[derive(Clone)]
pub(crate) struct TestHost {
    client: Arc<Client>,
}

impl Default for TestHost {
    fn default() -> Self {
        Self {
            client: Arc::new(Client::new()),
        }
    }
}

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

    async fn http_request(&self, req: HttpRequest) -> std::result::Result<HttpResponse, BoxError> {
        let mut request = http::Request::new(Full::new(req.body.unwrap_or_default()));
        *request.method_mut() = req.method;
        *request.uri_mut() = req.uri;
        *request.headers_mut() = req.headers;

        let response = self
            .client
            .send_http(request, RequestOptions::default())
            .await
            .map_err(|e| -> BoxError { Box::new(e) })?;

        Ok(response.map(|body| -> HttpBodyStream {
            Box::pin(body.map_err(|e| -> BoxError { Box::new(e) }))
        }))
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SinkState {
    pub(crate) partial: Vec<Bytes>,
    pub(crate) end: Vec<Bytes>,
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
pub(crate) struct TraceCollector {
    events: Arc<Mutex<Vec<EventRecord>>>,
}

impl TraceCollector {
    pub(crate) fn events(&self) -> Vec<EventRecord> {
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
    root.join("target").join("isola_python.wasm")
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

fn print_skip_once(message: String) {
    static SKIP_MESSAGE_ONCE: Once = Once::new();
    SKIP_MESSAGE_ONCE.call_once(|| {
        eprintln!("{message}");
    });
}

fn resolve_prereqs() -> Result<Option<(PathBuf, PathBuf)>> {
    let root = workspace_root()?;
    let wasm = bundle_path(&root);
    let lib_dir = resolve_lib_dir(&root);

    if !wasm.is_file() {
        print_skip_once(format!(
            "skipping integration_python tests: missing integration wasm bundle at '{}'. Build it with `cargo xtask build-all`.",
            wasm.display()
        ));
        return Ok(None);
    }

    if !lib_dir.is_dir() {
        print_skip_once(format!(
            "skipping integration_python tests: missing WASI runtime libraries at '{}'. Run in the dev shell or set WASI_PYTHON_RUNTIME, then build with `cargo xtask build-all`.",
            lib_dir.display()
        ));
        return Ok(None);
    }

    Ok(Some((wasm, lib_dir)))
}

fn build_module_lock() -> &'static tokio::sync::Mutex<()> {
    static BUILD_MODULE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    BUILD_MODULE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn build_module_with_policy() -> Result<Option<Module<TestHost>>> {
    // Serialize compilation because tests can run in parallel and share cache paths.
    let _build_guard = build_module_lock().lock().await;
    let Some((wasm, lib_dir)) = resolve_prereqs()? else {
        return Ok(None);
    };

    let builder = ModuleBuilder::new()
        .compile_config(CompileConfig {
            cache: CacheConfig::Default,
            ..CompileConfig::default()
        })
        .lib_dir(lib_dir);

    let module = builder
        .build(&wasm)
        .await
        .context("failed to build module from integration wasm bundle")?;

    Ok(Some(module))
}

pub(crate) async fn build_module() -> Result<Option<Module<TestHost>>> {
    build_module_with_policy().await
}

pub(crate) async fn call_collect(
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

pub(crate) fn cbor_arg(name: Option<&str>, json: &str) -> Result<Arg> {
    let value =
        json_to_cbor(json).with_context(|| format!("failed to convert json to cbor: {json}"))?;
    Ok(Arg {
        name: name.map(str::to_owned),
        value: ArgValue::Cbor(value),
    })
}
