use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use bytes::Bytes;
use futures::Stream;
use parking_lot::Mutex;
use promptkit::{Environment, Instance, Runtime};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;
use zip::ZipArchive;

use crate::utils::stream::join_with_infallible;

use super::env::{MpscOutputCallback, StreamItem};

pub enum Source {
    Script { prelude: String, code: String },
    Bundle(Bytes),
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

#[derive(Deserialize)]
struct Manifest {
    entrypoint: String,
    prelude: Option<String>,
}

struct CachedInstance<E: Environment> {
    instance: Instance<E>,
    _tempdir: Option<TempDir>,
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
                        lib_dir.push("wasm32-wasip2");
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
        match script {
            Source::Script { prelude, code } => {
                hasher.update(prelude);
                hasher.update(code);
            }
            Source::Bundle(b) => hasher.update(b),
        }
        hasher.finalize().into()
    }

    fn get_cached(&self, hash: [u8; 32]) -> Option<CachedInstance<E>> {
        self.cache.lock().get_mut(&hash)?.pop()
    }

    async fn extract_zip(data: impl AsRef<[u8]>, dest: &Path) -> anyhow::Result<()> {
        let data = data.as_ref().to_vec();
        let dest = dest.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let cursor = std::io::Cursor::new(data);
            let mut archive = ZipArchive::new(cursor)?;
            let mut dirs = std::collections::HashSet::new();
            let mut buffer = vec![0u8; 16384]; // 16KB buffer reused across files

            for i in 0..archive.len() {
                let mut file = archive.by_index(i)?;
                let outpath = match file.enclosed_name() {
                    Some(path) => dest.join(path),
                    None => continue,
                };

                if file.name().ends_with('/') {
                    // Directory
                    if dirs.insert(outpath.clone()) {
                        std::fs::create_dir_all(&outpath)?;
                    }
                } else {
                    // File
                    if let Some(p) = outpath.parent()
                        && !dirs.contains(p)
                    {
                        std::fs::create_dir_all(p)?;
                        dirs.insert(p.to_owned());
                    }

                    // Stream file contents in chunks
                    let mut out_file = std::fs::File::create(&outpath)?;
                    loop {
                        let bytes_read = std::io::Read::read(&mut file, &mut buffer)?;
                        if bytes_read == 0 {
                            break;
                        }
                        out_file.write_all(&buffer[..bytes_read])?;
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
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

        let (mut instance, tempdir) = match &script {
            Source::Script { .. } => {
                let instance = self.runtime.instantiate(None, env.clone()).await?;
                (instance, None)
            }
            Source::Bundle(bundle) => {
                let temp = TempDir::with_prefix("vm")?;
                let base = temp.path();
                Self::extract_zip(bundle, base).await?;

                let instance = self.runtime.instantiate(Some(base), env.clone()).await?;
                (instance, Some(temp))
            }
        };

        match script {
            Source::Script { prelude, code } => {
                if !prelude.is_empty() {
                    instance.eval_script(&prelude).await?;
                }
                instance.eval_script(&code).await?;
            }
            Source::Bundle(_) => {
                // Bundle was already extracted, now load the manifest and execute
                let temp_path = tempdir.as_ref().unwrap().path();
                let manifest: Manifest = serde_json::from_str(
                    &tokio::fs::read_to_string(temp_path.join("manifest.json")).await?,
                )?;

                if let Some(prelude) = &manifest.prelude {
                    instance.eval_script(prelude).await?;
                }

                instance.eval_file(&manifest.entrypoint).await?;
            }
        }

        Ok(CachedInstance {
            instance,
            _tempdir: tempdir,
        })
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
                    // Cache the instance for reuse (tempdir stays alive in the cache)
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
                            instances.remove(0); // Remove oldest (tempdir auto-cleaned)
                        }
                    }
                }
                Err(err) => {
                    _ = tx.send(StreamItem::Error(err)).await;
                    // Don't cache on error - let cached (and tempdir) be dropped
                }
            }
        });

        Ok(join_with_infallible(ReceiverStream::new(rx), task))
    }
}
