use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
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

const DEFAULT_PYTHON_TEMPLATE_PRELUDE: &str = "import sandbox.asyncio";
const DEFAULT_TEMPLATE_MAX_MEMORY: usize = 64 * 1024 * 1024;
const DEFAULT_CACHE_MAX_INSTANCES: usize = 64;
const DEFAULT_CACHE_TTL_MS: u64 = 60_000;

#[derive(Clone, Copy)]
enum RuntimeKind {
    Python,
    Js,
    Unknown,
}

impl RuntimeKind {
    fn from_wasm_path(path: &Path) -> Self {
        let file = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();

        if file.eq_ignore_ascii_case("js.wasm") || file.contains("js") {
            Self::Js
        } else if file.eq_ignore_ascii_case("python3.wasm") || file.contains("python") {
            Self::Python
        } else {
            Self::Unknown
        }
    }

    const fn default_template_prelude(self) -> Option<&'static str> {
        match self {
            Self::Js => None,
            Self::Python | Self::Unknown => Some(DEFAULT_PYTHON_TEMPLATE_PRELUDE),
        }
    }

    const fn requires_runtime_lib_dir(self) -> bool {
        match self {
            Self::Js => false,
            Self::Python | Self::Unknown => true,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Js => "js",
            Self::Unknown => "unknown",
        }
    }
}

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
    last_used: Instant,
}

type CacheMap<E> = Arc<Mutex<HashMap<[u8; 32], Vec<CachedSandbox<E>>>>>;

#[derive(Clone, Copy)]
struct CacheConfig {
    max_instances: usize,
    ttl: Option<Duration>,
}

struct ResolvedManagerConfig {
    runtime_kind: RuntimeKind,
    max_memory: usize,
    max_memory_source: &'static str,
    template_prelude: Option<String>,
    template_prelude_source: &'static str,
    template_cache_dir: Option<PathBuf>,
    template_cache_dir_source: &'static str,
    template_cache_dir_display: String,
    cache_max_instances: usize,
    cache_max_instances_source: &'static str,
    cache_ttl: Option<Duration>,
    cache_ttl_source: &'static str,
    lib_dir: Option<PathBuf>,
    runtime_lib_dir_source: &'static str,
    runtime_lib_dir_display: String,
}

pub struct SandboxManager<E: Host + Clone> {
    module: Arc<SandboxTemplate<E>>,
    cache: CacheMap<E>,
    cache_config: CacheConfig,
}

impl<E: Host + Clone> Clone for SandboxManager<E> {
    fn clone(&self) -> Self {
        Self {
            module: self.module.clone(),
            cache: self.cache.clone(),
            cache_config: self.cache_config,
        }
    }
}

impl<E: Host + Clone> SandboxManager<E> {
    #[allow(clippy::too_many_lines)]
    fn resolve_manager_config(parent: &Path, runtime_kind: RuntimeKind) -> ResolvedManagerConfig {
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
                Err(_) => (
                    runtime_kind
                        .default_template_prelude()
                        .map(ToString::to_string),
                    if runtime_kind.default_template_prelude().is_some() {
                        "default"
                    } else {
                        "default_runtime_none"
                    },
                ),
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

        let (cache_max_instances, cache_max_instances_source) =
            std::env::var("SANDBOX_CACHE_MAX_INSTANCES").map_or(
                (DEFAULT_CACHE_MAX_INSTANCES, "default"),
                |raw| match raw.parse::<usize>() {
                    Ok(parsed) => (parsed, "env"),
                    Err(err) => {
                        tracing::warn!(
                            %raw,
                            ?err,
                            "Invalid SANDBOX_CACHE_MAX_INSTANCES; falling back to default"
                        );
                        (DEFAULT_CACHE_MAX_INSTANCES, "default_invalid_env")
                    }
                },
            );

        let (cache_ttl, cache_ttl_source) = std::env::var("SANDBOX_CACHE_TTL_MS").map_or(
            (Some(Duration::from_millis(DEFAULT_CACHE_TTL_MS)), "default"),
            |raw| match raw.parse::<u64>() {
                Ok(0) => (None, "env_disabled"),
                Ok(ms) => (Some(Duration::from_millis(ms)), "env"),
                Err(err) => {
                    tracing::warn!(
                        %raw,
                        ?err,
                        "Invalid SANDBOX_CACHE_TTL_MS; falling back to default"
                    );
                    (
                        Some(Duration::from_millis(DEFAULT_CACHE_TTL_MS)),
                        "default_invalid_env",
                    )
                }
            },
        );

        let (lib_dir, runtime_lib_dir_source) = if runtime_kind.requires_runtime_lib_dir() {
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
            (Some(lib_dir), runtime_lib_dir_source)
        } else {
            (None, "not_required")
        };
        let runtime_lib_dir_display = lib_dir.as_ref().map_or_else(
            || "<disabled>".to_string(),
            |path| path.display().to_string(),
        );

        ResolvedManagerConfig {
            runtime_kind,
            max_memory,
            max_memory_source,
            template_prelude,
            template_prelude_source,
            template_cache_dir,
            template_cache_dir_source,
            template_cache_dir_display,
            cache_max_instances,
            cache_max_instances_source,
            cache_ttl,
            cache_ttl_source,
            lib_dir,
            runtime_lib_dir_source,
            runtime_lib_dir_display,
        }
    }

    pub async fn new(wasm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        info!("Creating SandboxManager...");
        let path = wasm_path.as_ref();
        let runtime_kind = RuntimeKind::from_wasm_path(path);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Wasm path has no parent directory"))?;
        let config = Self::resolve_manager_config(parent, runtime_kind);

        info!(
            wasm_path = %path.display(),
            runtime_kind = config.runtime_kind.as_str(),
            template_max_memory_bytes = config.max_memory,
            template_max_memory_source = config.max_memory_source,
            template_prelude_enabled = config.template_prelude.is_some(),
            template_prelude_source = config.template_prelude_source,
            template_cache_dir_source = config.template_cache_dir_source,
            runtime_lib_dir_source = config.runtime_lib_dir_source,
            runtime_lib_dir = %config.runtime_lib_dir_display,
            template_cache_dir = %config.template_cache_dir_display,
            cache_max_instances = config.cache_max_instances,
            cache_max_instances_source = config.cache_max_instances_source,
            cache_ttl_ms = config.cache_ttl.map_or(0, |ttl| ttl.as_millis()),
            cache_ttl_source = config.cache_ttl_source,
            "Resolved sandbox template configuration"
        );

        let mut module = SandboxTemplate::<E>::builder()
            .prelude(config.template_prelude)
            .cache(config.template_cache_dir)
            .max_memory(config.max_memory);

        if let Some(lib_dir) = &config.lib_dir {
            module = module.mount(lib_dir, "/lib", DirPerms::READ, FilePerms::READ);
        }

        let module = module.build(path).await?;

        Ok(Self {
            module: Arc::new(module),
            cache: Arc::new(Mutex::new(HashMap::new())),
            cache_config: CacheConfig {
                max_instances: config.cache_max_instances,
                ttl: config.cache_ttl,
            },
        })
    }

    fn compute_hash(id: &str, script: &Source) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(id);
        hasher.update(&script.prelude);
        hasher.update(&script.code);
        hasher.finalize().into()
    }

    fn prune_expired_cache(
        cache: &mut HashMap<[u8; 32], Vec<CachedSandbox<E>>>,
        ttl: Option<Duration>,
    ) {
        let Some(ttl) = ttl else {
            return;
        };

        cache.retain(|_, instances| {
            instances.retain(|cached| cached.last_used.elapsed() <= ttl);
            !instances.is_empty()
        });
    }

    fn evict_lru_until_limit(
        cache: &mut HashMap<[u8; 32], Vec<CachedSandbox<E>>>,
        max_instances: usize,
    ) {
        if max_instances == 0 {
            cache.clear();
            return;
        }

        while cache.values().map(Vec::len).sum::<usize>() > max_instances {
            let mut victim: Option<([u8; 32], usize, Instant)> = None;
            for (key, instances) in cache.iter() {
                for (idx, cached) in instances.iter().enumerate() {
                    if victim.is_none_or(|(_, _, ts)| cached.last_used < ts) {
                        victim = Some((*key, idx, cached.last_used));
                    }
                }
            }

            let Some((victim_key, victim_idx, _)) = victim else {
                break;
            };

            if let Some(instances) = cache.get_mut(&victim_key) {
                instances.swap_remove(victim_idx);
                if instances.is_empty() {
                    cache.remove(&victim_key);
                }
            }
        }
    }

    fn get_cached(&self, hash: [u8; 32]) -> Option<CachedSandbox<E>> {
        let mut cache = self.cache.lock();
        Self::prune_expired_cache(&mut cache, self.cache_config.ttl);
        let cached = cache.get_mut(&hash)?.pop();
        if cache.get(&hash).is_some_and(Vec::is_empty) {
            cache.remove(&hash);
        }
        cached
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

        Ok(CachedSandbox {
            sandbox,
            last_used: Instant::now(),
        })
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
        let cache_config = self.cache_config;
        let task = Box::pin(async move {
            let result = tokio::time::timeout(
                timeout,
                cached.sandbox.call_with_sink(&func, args, Arc::new(sink)),
            )
            .await
            .unwrap_or_else(|_| {
                Err(isola::sandbox::Error::Other(
                    anyhow::anyhow!(
                    "sandbox call timed out after {}ms",
                    timeout.as_millis()
                )
                    .into()))
            });
            match result {
                Ok(()) => {
                    cached.last_used = Instant::now();
                    let mut cache_lock = cache.lock();
                    cache_lock.entry(hash).or_default().push(cached);
                    Self::prune_expired_cache(&mut cache_lock, cache_config.ttl);
                    Self::evict_lru_until_limit(&mut cache_lock, cache_config.max_instances);
                    drop(cache_lock);
                }
                Err(err) => {
                    let _ = tx.send(StreamItem::Error(err));
                }
            }
        });

        Ok(join_with_infallible(UnboundedReceiverStream::new(rx), task))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::RuntimeKind;

    #[test]
    fn detects_js_runtime_from_default_bundle_name() {
        let runtime = RuntimeKind::from_wasm_path(Path::new("target/js.wasm"));
        assert!(matches!(runtime, RuntimeKind::Js));
    }

    #[test]
    fn detects_python_runtime_from_default_bundle_name() {
        let runtime = RuntimeKind::from_wasm_path(Path::new("target/python3.wasm"));
        assert!(matches!(runtime, RuntimeKind::Python));
    }

    #[test]
    fn js_runtime_disables_python_template_prelude() {
        let runtime = RuntimeKind::from_wasm_path(Path::new("target/js.wasm"));
        assert_eq!(runtime.default_template_prelude(), None);
        assert!(!runtime.requires_runtime_lib_dir());
    }
}
