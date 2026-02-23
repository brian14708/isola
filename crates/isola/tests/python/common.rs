use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Once, OnceLock},
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::TryStreamExt;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse},
    sandbox::{DirPerms, FilePerms, SandboxTemplate},
    value::Value,
};
use isola_request::{Client, RequestOptions};

#[derive(Clone)]
pub struct TestHost {
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
        payload: Value,
    ) -> std::result::Result<Value, BoxError> {
        match call_type {
            "echo" => Ok(payload),
            _ => Err(std::io::Error::other(format!("unsupported hostcall: {call_type}")).into()),
        }
    }

    async fn http_request(&self, req: HttpRequest) -> std::result::Result<HttpResponse, BoxError> {
        let mut request = http::Request::new(req.body().clone().unwrap_or_default());
        *request.method_mut() = req.method().clone();
        *request.uri_mut() = req.uri().clone();
        *request.headers_mut() = req.headers().clone();

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

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("failed to resolve workspace root from CARGO_MANIFEST_DIR")
}

fn bundle_path(root: &Path) -> PathBuf {
    root.join("target").join("python3.wasm")
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

fn print_skip_once(message: &str) {
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
        let message = format!(
            "skipping integration_python tests: missing integration wasm bundle at '{}'. Build it with `cargo xtask build-all`.",
            wasm.display()
        );
        print_skip_once(&message);
        return Ok(None);
    }

    if !lib_dir.is_dir() {
        let message = format!(
            "skipping integration_python tests: missing WASI runtime libraries at '{}'. Run in the dev shell or set WASI_PYTHON_RUNTIME, then build with `cargo xtask build-all`.",
            lib_dir.display()
        );
        print_skip_once(&message);
        return Ok(None);
    }

    Ok(Some((wasm, lib_dir)))
}

fn build_module_lock() -> &'static tokio::sync::Mutex<()> {
    static BUILD_MODULE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    BUILD_MODULE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn build_module_with_policy(
    max_memory: Option<usize>,
) -> Result<Option<SandboxTemplate<TestHost>>> {
    // Serialize compilation because tests can run in parallel and share cache
    // paths.
    let _build_guard = build_module_lock().lock().await;
    let Some((wasm, lib_dir)) = resolve_prereqs()? else {
        return Ok(None);
    };
    let cache_dir = wasm
        .parent()
        .ok_or_else(|| anyhow::anyhow!("integration wasm bundle has no parent directory"))?
        .join("cache");

    let mut builder = SandboxTemplate::<TestHost>::builder()
        .prelude(Some("import sandbox.asyncio".to_string()))
        .cache(Some(cache_dir))
        .mount(&lib_dir, "/lib", DirPerms::READ, FilePerms::READ);
    if let Some(max_memory) = max_memory {
        builder = builder.max_memory(max_memory);
    }

    let module = builder
        .build(&wasm)
        .await
        .context("failed to build module from integration wasm bundle")?;

    Ok(Some(module))
}

pub async fn build_module() -> Result<Option<SandboxTemplate<TestHost>>> {
    build_module_with_policy(None).await
}

pub async fn build_module_with_max_memory(
    max_memory: usize,
) -> Result<Option<SandboxTemplate<TestHost>>> {
    build_module_with_policy(Some(max_memory)).await
}
