use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use bytes::Bytes;
use futures::Stream;
use parking_lot::Mutex;
use promptkit::{Environment, Instance, Runtime};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;

use crate::utils::stream::join_with_infallible;

use super::env::{MpscOutputCallback, StreamItem};

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

    fn into_promptkit(self, name: Option<String>) -> (Option<String>, promptkit::Argument) {
        match self {
            Self::Cbor(data) => (name, promptkit::Argument::Cbor(data)),
            Self::CborStream(stream) => (name, promptkit::Argument::CborStream(stream)),
        }
    }
}

struct CachedInstance<E: Environment> {
    instance: Instance<E>,
}

type CacheMap<E> = Arc<Mutex<HashMap<[u8; 32], Vec<CachedInstance<E>>>>>;

pub struct VmManager<E: Environment> {
    runtime: Arc<Runtime<E>>,
    cache: CacheMap<E>,
}

impl<E: Environment> Clone for VmManager<E> {
    fn clone(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
            cache: self.cache.clone(),
        }
    }
}

impl<E: Environment> VmManager<E> {
    pub async fn new(wasm_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        info!("Creating VmManager...");
        let path = wasm_path.as_ref();
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Wasm path has no parent directory"))?;

        let runtime = Runtime::<E>::builder()
            .max_memory(
                std::env::var("VM_MAX_MEMORY")
                    .ok()
                    .and_then(|f| f.parse().ok())
                    .unwrap_or(64 * 1024 * 1024),
            )
            .compile_prelude("import promptkit.asyncio")
            .cache_path(parent.join("cache"))
            .library_path({
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
                lib_dir
            })
            .build(path)
            .await?;

        Ok(Self {
            runtime: Arc::new(runtime),
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

    fn get_cached(&self, hash: [u8; 32]) -> Option<CachedInstance<E>> {
        self.cache.lock().get_mut(&hash)?.pop()
    }

    async fn prepare_instance(
        &self,
        hash: [u8; 32],
        script: Source,
        env: E,
    ) -> anyhow::Result<CachedInstance<E>> {
        if let Some(cached) = self.get_cached(hash) {
            return Ok(cached);
        }

        let mut instance = self.runtime.instantiate(None, env.clone()).await?;

        if !script.prelude.is_empty() {
            instance.eval_script(&script.prelude).await?;
        }
        instance.eval_script(&script.code).await?;

        Ok(CachedInstance { instance })
    }

    pub async fn exec(
        &self,
        id: &str,
        script: Source,
        func: String,
        args: Vec<(Option<String>, Argument)>,
        env: E,
    ) -> anyhow::Result<impl Stream<Item = StreamItem> + use<E>>
    where
        E::Callback: From<MpscOutputCallback>,
    {
        let hash = Self::compute_hash(id, &script);
        let mut cached = self.prepare_instance(hash, script, env.clone()).await?;

        let (tx, rx) = mpsc::channel(4);
        let callback = E::Callback::from(MpscOutputCallback::new(tx.clone()));

        let args: Vec<_> = args
            .into_iter()
            .map(|(name, a)| a.into_promptkit(name))
            .collect();

        let cache = self.cache.clone();
        let task = Box::pin(async move {
            let result = cached.instance.execute(&func, args, callback).await;
            match result {
                Ok(()) => {
                    // Cache the instance for reuse
                    let mut cache_lock = cache.lock();
                    cache_lock.entry(hash).or_default().push(cached);

                    // Limit cache size to prevent unbounded growth
                    let total: usize = cache_lock.values().map(Vec::len).sum();
                    if total > 64 {
                        // Remove oldest instance from a random key
                        if let Some(key) = cache_lock.keys().next().copied()
                            && let Some(instances) = cache_lock.get_mut(&key)
                            && !instances.is_empty()
                        {
                            instances.remove(0);
                        }
                    }
                }
                Err(err) => {
                    _ = tx.send(StreamItem::Error(err)).await;
                }
            }
        });

        Ok(join_with_infallible(ReceiverStream::new(rx), task))
    }
}
