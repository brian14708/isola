use std::{
    future::Future,
    io::Cursor,
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use anyhow::anyhow;
use pin_project_lite::pin_project;
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use tokio::{io::AsyncWriteExt, sync::mpsc, task::JoinHandle};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tracing::info;
use wasmtime::{
    component::{Component, InstancePre, ResourceTableError},
    Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig,
};

use crate::{
    trace::BoxedTracer,
    vm::{exports::Argument, Sandbox, Vm, VmState},
    vm_cache::VmCache,
};

const MAX_MEMORY: usize = 64 * 1024 * 1024;
const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct VmManager {
    engine: Engine,
    instance_pre: InstancePre<VmState>,
    cache: Arc<VmCache>,
    epoch_ticker: JoinHandle<()>,
}

#[derive(Debug)]
pub enum ExecStreamItem {
    Data(Vec<u8>),
    End(Option<Vec<u8>>),
    Error(anyhow::Error),
}

pub enum ExecArgument {
    Cbor(Vec<u8>),
    CborStream(mpsc::Receiver<Vec<u8>>),
}

pub enum ExecSource<'a> {
    Script(&'a str),
    Bundle(&'a [u8]),
}

#[derive(serde::Deserialize)]
struct Manifest {
    entrypoint: String,
}

impl<'a> ExecSource<'a> {
    fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        match self {
            Self::Script(s) => hasher.update(s),
            Self::Bundle(b) => hasher.update(b),
        }
        hasher.finalize().into()
    }
}

impl VmManager {
    fn cfg() -> Config {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .epoch_interruption(true)
            .cranelift_opt_level(wasmtime::OptLevel::Speed);

        let mut pooling_config = PoolingAllocationConfig::default();
        pooling_config.memory_pages(64 * 1024 * 1024 / (64 * 1024));
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pooling_config));

        config
    }

    pub fn compile(path: &Path) -> anyhow::Result<()> {
        let config = Self::cfg();
        let engine = Engine::new(&config)?;
        let component = Component::from_file(&engine, path)?;
        let data = component.serialize()?;
        std::fs::write(path.with_extension("wasm.cache"), data)?;
        Ok(())
    }

    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let config = Self::cfg();
        let engine = Engine::new(&config)?;

        info!("Loading module...");
        let component = {
            let cache_path = path.with_extension("wasm.cache");

            let mod_time = std::fs::metadata(path)?.modified()?;
            let cache = std::fs::metadata(&cache_path)
                .map_err(anyhow::Error::from)
                .and_then(|v| {
                    if mod_time <= v.modified()? {
                        Ok(unsafe { Component::deserialize_file(&engine, &cache_path)? })
                    } else {
                        Err(anyhow!("cache is outdated"))
                    }
                });

            if let Ok(c) = cache {
                c
            } else {
                let component = Component::from_file(&engine, path)?;
                #[cfg(debug_assertions)]
                {
                    let data = component.serialize()?;
                    std::fs::write(cache_path, data)?;
                }
                component
            }
        };

        let linker = VmState::new_linker(&engine)?;
        let instance_pre = linker.instantiate_pre(&component)?;

        info!("Loaded module!");

        Ok(Self {
            engine: engine.clone(),
            instance_pre,
            cache: Arc::new(VmCache::new()),
            epoch_ticker: tokio::task::spawn(async move {
                let mut interval = tokio::time::interval(EPOCH_TICK);
                loop {
                    interval.tick().await;
                    engine.increment_epoch();
                }
            }),
        })
    }

    pub async fn create(&self, hash: [u8; 32]) -> anyhow::Result<Vm> {
        let workdir = tempdir::TempDir::new("vm").map_err(anyhow::Error::from)?;
        let mut store = VmState::new(&self.engine, workdir.path(), MAX_MEMORY);
        store.epoch_deadline_async_yield_and_update(1);

        let (bindings, _) = Sandbox::instantiate_pre(&mut store, &self.instance_pre).await?;
        Ok(Vm {
            hash,
            store,
            sandbox: bindings,
            workdir,
        })
    }

    fn exec_impl(
        &self,
        func: &str,
        args: SmallVec<[Argument; 2]>,
        tracer: Option<BoxedTracer>,
        vm: Vm,
    ) -> impl Stream<Item = ExecStreamItem> + Send + 'static {
        let (tx, rx) = mpsc::channel(4);
        let cache = self.cache.clone();

        let mut run = vm.run(tracer, tx.clone());
        let func = func.to_string();
        let exec = Some(Box::pin(async move {
            let ret = run
                .exec(|vm, store| vm.call_call_func(store, &func, &args))
                .await
                .and_then(|v| {
                    v.map_err(|e| match e {
                        crate::vm::exports::Error::Code(err) => anyhow!("[Code] {0}", err),
                        crate::vm::exports::Error::Unknown(err) => anyhow!("[Unknown] {0}", err),
                    })
                });
            match ret {
                Ok(e) => {
                    let _ = tx.send(Ok((e.unwrap_or_default(), true))).await;
                    cache.put(run.reuse());
                }
                Err(err) => {
                    _ = tx.send(Err(err)).await;
                }
            };
        }));

        join_with(
            ReceiverStream::new(rx).map(|v| match v {
                Ok((data, true)) => {
                    ExecStreamItem::End(if data.is_empty() { None } else { Some(data) })
                }
                Ok((data, false)) => ExecStreamItem::Data(data),
                Err(err) => ExecStreamItem::Error(err),
            }),
            exec,
        )
    }

    pub async fn exec(
        &'_ self,
        script: ExecSource<'_>,
        func: &str,
        args: impl IntoIterator<Item = ExecArgument>,
        tracer: Option<BoxedTracer>,
    ) -> anyhow::Result<impl Stream<Item = ExecStreamItem> + Send + 'static> {
        let hash = script.hash();

        let vm = self.cache.get(hash);
        let mut vm = if let Some(vm) = vm {
            vm
        } else {
            let mut vm = self.create(hash).await?;
            match script {
                ExecSource::Script(script) => {
                    vm.sandbox
                        .promptkit_script_guest_api()
                        .call_eval_script(&mut vm.store, script)
                        .await?
                        .map_err(|e| match e {
                            crate::vm::exports::Error::Code(err) => anyhow!("[Code] {0}", err),
                            crate::vm::exports::Error::Unknown(err) => {
                                anyhow!("[Unknown] {0}", err)
                            }
                        })?;
                }
                ExecSource::Bundle(bundle) => {
                    let mut zip = zip::ZipArchive::new(Cursor::new(bundle))?;
                    let manifest: Manifest = serde_json::from_reader(
                        zip.by_name("manifest.json").map_err(anyhow::Error::from)?,
                    )?;

                    let b = vm.workdir.path().join("bundle.zip");
                    let mut file = tokio::fs::File::options()
                        .create(true)
                        .write(true)
                        .open(&b)
                        .await?;
                    file.write_all(bundle).await?;
                    drop(file);

                    vm.sandbox
                        .promptkit_script_guest_api()
                        .call_eval_bundle(
                            &mut vm.store,
                            "/workdir/bundle.zip",
                            &manifest.entrypoint,
                        )
                        .await?
                        .map_err(|e| match e {
                            crate::vm::exports::Error::Code(err) => anyhow!("[Code] {0}", err),
                            crate::vm::exports::Error::Unknown(err) => {
                                anyhow!("[Unknown] {0}", err)
                            }
                        })?;
                }
            }
            vm
        };
        Ok(self.exec_impl(
            func,
            args.into_iter()
                .map::<Result<_, ResourceTableError>, _>(|a| match a {
                    ExecArgument::Cbor(a) => Ok(Argument::Cbor(a)),
                    ExecArgument::CborStream(s) => Ok(Argument::Iterator(
                        vm.new_iter(Box::pin(ReceiverStream::new(s).map(Argument::Cbor)))?,
                    )),
                })
                .collect::<Result<_, _>>()?,
            tracer,
            vm,
        ))
    }
}

impl Drop for VmManager {
    fn drop(&mut self) {
        self.epoch_ticker.abort();
        // yield one last time
        self.engine.increment_epoch();
    }
}

fn join_with<T>(
    stream: impl Stream<Item = T>,
    task: Option<impl Future<Output = ()>>,
) -> impl Stream<Item = T> {
    StreamJoin {
        stream: Some(stream),
        task,
    }
}

pin_project! {
    pub struct StreamJoin<S: Stream<Item = T>, F: Future<Output = ()>, T> {
        #[pin]
        stream: Option<S>,
        #[pin]
        task: Option<F>,
    }
}

impl<S: Stream<Item = T>, F: Future<Output = ()>, T> Stream for StreamJoin<S, F, T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if let Some(stream) = this.stream.as_mut().as_pin_mut() {
            match stream.poll_next(cx) {
                Poll::Ready(None) => this.stream.set(None),
                v @ Poll::Ready(Some(_)) => return v,
                Poll::Pending => {}
            }
        }

        if let Some(task) = this.task.as_mut().as_pin_mut() {
            if task.poll(cx) == Poll::Ready(()) {
                this.task.set(None);
            }
        }

        if this.stream.is_none() && this.task.is_none() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}
