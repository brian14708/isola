use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use futures::Stream;
use isola::{
    host::Host,
    sandbox::{Arg, DirPerms, FilePerms, Sandbox, SandboxOptions, SandboxTemplate},
    value::Value,
};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::info;

use super::env::{MpscOutputSink, StreamItem};
use crate::utils::stream::join_with_infallible;

const DEFAULT_TEMPLATE_PRELUDE: &str = "import sandbox.asyncio";
const DEFAULT_TEMPLATE_MAX_MEMORY: usize = 64 * 1024 * 1024;

pub struct Source {
    pub prelude: String,
    pub code: String,
}

#[derive(Clone, Copy)]
pub struct ExecOptions {
    pub timeout: Duration,
}

pub enum Argument {
    Value(Value),
    Stream(Pin<Box<dyn Stream<Item = Value> + Send>>),
}

impl Argument {
    #[must_use]
    pub fn value(value: impl Into<Value>) -> Self {
        Self::Value(value.into())
    }

    #[must_use]
    pub fn stream(value: impl Stream<Item = Value> + Send + 'static) -> Self {
        Self::Stream(Box::pin(value))
    }

    fn into_isola(self, name: Option<String>) -> Arg {
        match (name, self) {
            (Some(name), Self::Value(data)) => Arg::Named(name, data),
            (None, Self::Value(data)) => Arg::Positional(data),
            (Some(name), Self::Stream(stream)) => Arg::NamedStream(name, stream),
            (None, Self::Stream(stream)) => Arg::PositionalStream(stream),
        }
    }
}

struct CachedSandbox<E: Host + Clone> {
    sandbox: Sandbox<E>,
}

type CacheMap<E> = Arc<Mutex<HashMap<[u8; 32], Vec<CachedSandbox<E>>>>>;

pub struct SandboxManager<E: Host + Clone> {
    module: Arc<SandboxTemplate<E>>,
    cache: CacheMap<E>,
}

impl<E: Host + Clone> Clone for SandboxManager<E> {
    fn clone(&self) -> Self {
        Self {
            module: self.module.clone(),
            cache: self.cache.clone(),
        }
    }
}

impl<E: Host + Clone> SandboxManager<E> {
    pub async fn new(wasm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        info!("Creating SandboxManager...");
        let path = wasm_path.as_ref();
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Wasm path has no parent directory"))?;

        let (max_memory, max_memory_source) = std::env::var("SANDBOX_MAX_MEMORY").map_or(
            (DEFAULT_TEMPLATE_MAX_MEMORY, "default"),
            |raw| match raw.parse::<usize>() {
                Ok(parsed) => (parsed, "env"),
                Err(err) => {
                    tracing::warn!(
                        %raw,
                        ?err,
                        "Invalid SANDBOX_MAX_MEMORY; falling back to default"
                    );
                    (DEFAULT_TEMPLATE_MAX_MEMORY, "default_invalid_env")
                }
            },
        );

        let (template_prelude, template_prelude_source) =
            match std::env::var("SANDBOX_TEMPLATE_PRELUDE") {
                Ok(value) if value.is_empty() => (None, "env_empty"),
                Ok(value) => (Some(value), "env"),
                Err(_) => (Some(DEFAULT_TEMPLATE_PRELUDE.to_string()), "default"),
            };

        let (template_cache_dir, template_cache_dir_source) =
            match std::env::var("SANDBOX_TEMPLATE_CACHE_DIR") {
                Ok(value) if value.trim().is_empty() => (None, "env_empty"),
                Ok(value) => (Some(PathBuf::from(value)), "env"),
                Err(_) => (Some(parent.join("cache")), "derived"),
            };
        let template_cache_dir_display = template_cache_dir.as_ref().map_or_else(
            || "<disabled>".to_string(),
            |path| path.display().to_string(),
        );

        let (mut lib_dir, runtime_lib_dir_source) = std::env::var("WASI_PYTHON_RUNTIME")
            .map_or_else(
                |_| {
                    let mut lib_dir = parent.to_owned();
                    lib_dir.push("wasm32-wasip1");
                    lib_dir.push("wasi-deps");
                    lib_dir.push("usr");
                    lib_dir.push("local");
                    (lib_dir, "derived")
                },
                |path| (PathBuf::from(path), "env"),
            );
        lib_dir.push("lib");

        info!(
            wasm_path = %path.display(),
            template_max_memory_bytes = max_memory,
            template_max_memory_source = max_memory_source,
            template_prelude_enabled = template_prelude.is_some(),
            template_prelude_source,
            template_cache_dir_source,
            runtime_lib_dir_source,
            runtime_lib_dir = %lib_dir.display(),
            template_cache_dir = %template_cache_dir_display,
            "Resolved sandbox template configuration"
        );

        let module = SandboxTemplate::<E>::builder()
            .prelude(template_prelude)
            .cache(template_cache_dir)
            .max_memory(max_memory)
            .mount(&lib_dir, "/lib", DirPerms::READ, FilePerms::READ)
            .build(path)
            .await?;

        Ok(Self {
            module: Arc::new(module),
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn compute_hash(id: &str, script: &Source) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(id);
        hasher.update(&script.prelude);
        hasher.update(&script.code);
        hasher.finalize().into()
    }

    fn get_cached(&self, hash: [u8; 32]) -> Option<CachedSandbox<E>> {
        self.cache.lock().get_mut(&hash)?.pop()
    }

    async fn prepare_sandbox(
        &self,
        hash: [u8; 32],
        script: Source,
        env: E,
        sink: MpscOutputSink,
    ) -> anyhow::Result<CachedSandbox<E>> {
        if let Some(cached) = self.get_cached(hash) {
            return Ok(cached);
        }

        let mut sandbox = self
            .module
            .instantiate(env, SandboxOptions::default())
            .await?;

        if !script.prelude.is_empty() {
            sandbox
                .eval_script(&script.prelude, Arc::new(sink.clone()))
                .await?;
        }
        sandbox.eval_script(&script.code, Arc::new(sink)).await?;

        Ok(CachedSandbox { sandbox })
    }

    pub async fn exec(
        &self,
        id: &str,
        script: Source,
        func: String,
        args: Vec<(Option<String>, Argument)>,
        env: E,
        options: ExecOptions,
    ) -> anyhow::Result<impl Stream<Item = StreamItem> + use<E>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sink = MpscOutputSink::new(tx.clone());
        let hash = Self::compute_hash(id, &script);
        let mut cached = self
            .prepare_sandbox(hash, script, env.clone(), sink.clone())
            .await?;

        let args: Vec<_> = args
            .into_iter()
            .map(|(name, argument)| argument.into_isola(name))
            .collect();

        let timeout = options.timeout;
        let cache = self.cache.clone();
        let task = Box::pin(async move {
            let result = tokio::time::timeout(
                timeout,
                cached.sandbox.call_with_sink(&func, args, Arc::new(sink)),
            )
            .await
            .unwrap_or_else(|_| {
                Err(isola::sandbox::Error::Runtime(anyhow::anyhow!(
                    "sandbox call timed out after {}ms",
                    timeout.as_millis()
                )))
            });
            match result {
                Ok(()) => {
                    let mut cache_lock = cache.lock();
                    cache_lock.entry(hash).or_default().push(cached);

                    let total: usize = cache_lock.values().map(Vec::len).sum();
                    if total > 64
                        && let Some(key) = cache_lock.keys().next().copied()
                        && let Some(instances) = cache_lock.get_mut(&key)
                        && !instances.is_empty()
                    {
                        instances.remove(0);
                    }
                }
                Err(err) => {
                    let _ = tx.send(StreamItem::Error(err));
                }
            }
        });

        Ok(join_with_infallible(UnboundedReceiverStream::new(rx), task))
    }
}
