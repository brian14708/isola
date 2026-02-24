use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Once, OnceLock},
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::TryStreamExt;
use http::header::HOST;
use isola::{
    host::{BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse},
    sandbox::SandboxTemplate,
    value::Value,
};
use reqwest::Client;

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
        let mut headers = req.headers().clone();
        headers.remove(HOST);

        let response = self
            .client
            .request(req.method().clone(), req.uri().to_string())
            .headers(headers)
            .body(req.body().clone().unwrap_or_default())
            .send()
            .await
            .map_err(|e| -> BoxError { Box::new(e) })?;

        let mut builder = http::Response::builder()
            .status(response.status())
            .version(response.version());
        if let Some(headers) = builder.headers_mut() {
            *headers = response.headers().clone();
        }

        let body = response
            .bytes_stream()
            .map_ok(http_body::Frame::data)
            .map_err(|e| -> BoxError { Box::new(e) });

        builder
            .body(Box::pin(body) as HttpBodyStream)
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)
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
    root.join("target").join("js.wasm")
}

fn print_skip_once(message: &str) {
    static SKIP_MESSAGE_ONCE: Once = Once::new();
    SKIP_MESSAGE_ONCE.call_once(|| {
        eprintln!("{message}");
    });
}

fn resolve_prereqs() -> Result<Option<PathBuf>> {
    let root = workspace_root()?;
    let wasm = bundle_path(&root);

    if !wasm.is_file() {
        let message = format!(
            "skipping integration_js tests: missing JS wasm bundle at '{}'. Build it with `cargo xtask build-js`.",
            wasm.display()
        );
        print_skip_once(&message);
        return Ok(None);
    }

    Ok(Some(wasm))
}

fn build_module_lock() -> &'static tokio::sync::Mutex<()> {
    static BUILD_MODULE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    BUILD_MODULE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn build_module_with_policy(
    max_memory: Option<usize>,
) -> Result<Option<SandboxTemplate<TestHost>>> {
    let _build_guard = build_module_lock().lock().await;
    let Some(wasm) = resolve_prereqs()? else {
        return Ok(None);
    };
    let cache_dir = wasm
        .parent()
        .ok_or_else(|| anyhow::anyhow!("integration wasm bundle has no parent directory"))?
        .join("cache");

    let mut builder = SandboxTemplate::<TestHost>::builder().cache(Some(cache_dir));
    if let Some(max_memory) = max_memory {
        builder = builder.max_memory(max_memory);
    }

    let module = builder
        .build(&wasm)
        .await
        .context("failed to build module from JS integration wasm bundle")?;

    Ok(Some(module))
}

pub async fn build_module() -> Result<Option<SandboxTemplate<TestHost>>> {
    build_module_with_policy(None).await
}
