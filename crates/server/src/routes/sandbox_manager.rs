use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use futures::Stream;
use isola::{
    AclPolicyBuilder, CacheConfig, CallOptions, CompileConfig, Host, Module, ModuleBuilder,
    module::ArgValue,
    net::{AclRule, AclScheme},
};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;

use crate::utils::stream::join_with_infallible;

use super::env::{MpscOutputSink, StreamItem};

pub struct Source {
    pub prelude: String,
    pub code: String,
}

pub enum Argument {
    Cbor(Bytes),
    CborStream(Pin<Box<dyn Stream<Item = Bytes> + Send>>),
}

impl Argument {
    #[must_use]
    pub fn cbor(value: impl Into<Bytes>) -> Self {
        Self::Cbor(value.into())
    }

    #[must_use]
    pub fn cbor_stream(value: impl Stream<Item = Bytes> + Send + 'static) -> Self {
        Self::CborStream(Box::pin(value))
    }

    fn into_isola(self, name: Option<String>) -> isola::Arg {
        isola::Arg {
            name,
            value: match self {
                Self::Cbor(data) => ArgValue::Cbor(data),
                Self::CborStream(stream) => ArgValue::CborStream(stream),
            },
        }
    }
}

struct CachedSandbox<E: Host + Clone> {
    sandbox: isola::Sandbox<E>,
}

type CacheMap<E> = Arc<Mutex<HashMap<[u8; 32], Vec<CachedSandbox<E>>>>>;

pub struct SandboxManager<E: Host + Clone> {
    module: Arc<Module<E>>,
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

        let max_memory = std::env::var("SANDBOX_MAX_MEMORY")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or(64 * 1024 * 1024);

        let mut lib_dir = std::env::var("WASI_PYTHON_RUNTIME").map_or_else(
            |_| {
                let mut lib_dir = parent.to_owned();
                lib_dir.push("wasm32-wasip1");
                lib_dir.push("wasi-deps");
                lib_dir.push("usr");
                lib_dir.push("local");
                lib_dir
            },
            PathBuf::from,
        );
        lib_dir.push("lib");

        let module = ModuleBuilder::new()
            .compile_config(CompileConfig {
                cache: CacheConfig::Default,
                max_memory,
                ..CompileConfig::default()
            })
            .network_policy(Arc::new(
                AclPolicyBuilder::new()
                    .deny_private_ranges(false)
                    .push(AclRule::allow().schemes([
                        AclScheme::Http,
                        AclScheme::Https,
                        AclScheme::Ws,
                        AclScheme::Wss,
                    ]))
                    .build(),
            ))
            .lib_dir(lib_dir)
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
    ) -> anyhow::Result<CachedSandbox<E>> {
        if let Some(cached) = self.get_cached(hash) {
            return Ok(cached);
        }

        let mut sandbox = self.module.instantiate(None, env).await?;

        if !script.prelude.is_empty() {
            sandbox.eval_script(&script.prelude).await?;
        }
        sandbox.eval_script(&script.code).await?;

        Ok(CachedSandbox { sandbox })
    }

    pub async fn exec(
        &self,
        id: &str,
        script: Source,
        func: String,
        args: Vec<(Option<String>, Argument)>,
        timeout: Duration,
        env: E,
    ) -> anyhow::Result<impl Stream<Item = StreamItem> + use<E>> {
        let hash = Self::compute_hash(id, &script);
        let mut cached = self.prepare_sandbox(hash, script, env.clone()).await?;

        let (tx, rx) = mpsc::channel(4);
        let sink = MpscOutputSink::new(tx.clone());

        let args: Vec<_> = args
            .into_iter()
            .map(|(name, argument)| argument.into_isola(name))
            .collect();

        let cache = self.cache.clone();
        let task = Box::pin(async move {
            let result = cached
                .sandbox
                .call(&func, args, sink, CallOptions::default().timeout(timeout))
                .await;
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
                    let _ = tx.send(StreamItem::Error(err)).await;
                }
            }
        });

        Ok(join_with_infallible(ReceiverStream::new(rx), task))
    }
}
