use std::{
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use anyhow::anyhow;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tracing::info;
use wasmtime::{
    component::{Component, InstancePre},
    Config, Engine,
};

use crate::{
    vm::{exports::Argument, PythonVm, Vm, VmState},
    vm_cache::VmCache,
};

const MAX_MEMORY: usize = 64 * 1024 * 1024;
const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct VmManager {
    engine: Engine,
    instance_pre: InstancePre<VmState>,
    cache: Arc<VmCache>,
    _epoch_ticker: ChildTask<()>,
}

pub enum ExecStreamItem {
    Data(String),
    End(Option<String>),
    Error(anyhow::Error),
}

pub enum ExecResult {
    Error(anyhow::Error),
    Response(String),
    Stream(Pin<Box<dyn tokio_stream::Stream<Item = ExecStreamItem> + Send>>),
}

impl VmManager {
    fn cfg() -> Config {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .epoch_interruption(true)
            .cranelift_opt_level(wasmtime::OptLevel::Speed);
        config
    }

    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let config = Self::cfg();
        let engine = Engine::new(&config)?;

        info!("Loading module...");
        #[cfg(debug_assertions)]
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

            match cache {
                Ok(c) => c,
                Err(_) => {
                    let component = Component::from_file(&engine, path)?;
                    let data = component.serialize()?;
                    std::fs::write(cache_path, data)?;
                    component
                }
            }
        };

        #[cfg(not(debug_assertions))]
        let component = Component::from_file(&engine, path)?;

        let linker = VmState::new_linker(&engine)?;
        let instance_pre = linker.instantiate_pre(&component)?;

        info!("Loaded module!");

        Ok(Self {
            engine: engine.clone(),
            instance_pre,
            cache: Arc::new(VmCache::new()),
            _epoch_ticker: ChildTask::spawn(async move {
                let mut interval = tokio::time::interval(EPOCH_TICK);
                loop {
                    interval.tick().await;
                    engine.increment_epoch();
                }
            }),
        })
    }

    pub async fn create(&self, hash: [u8; 32]) -> anyhow::Result<Vm> {
        let mut store = VmState::new(&self.engine, MAX_MEMORY);
        store.epoch_deadline_async_yield_and_update(1);

        let (bindings, _) = PythonVm::instantiate_pre(&mut store, &self.instance_pre).await?;
        Ok(Vm {
            hash,
            store,
            python: bindings,
        })
    }

    async fn exec_impl(
        &self,
        func: String,
        args: Vec<String>,
        mut vm: Vm,
    ) -> anyhow::Result<ExecResult> {
        let (tx, mut rx) = mpsc::channel(4);
        vm.store.data_mut().initialize(tx.clone());

        let cache_weak = Arc::downgrade(&self.cache);
        let exec = ChildTask::spawn(async move {
            let ret = vm
                .python
                .vm()
                .call_call_func(
                    &mut vm.store,
                    &func,
                    &args.into_iter().map(Argument::Json).collect::<Vec<_>>(),
                )
                .await
                .and_then(|v| v.map_err(|e| anyhow!(e)));
            match ret {
                Ok(()) => {
                    if let Some(cache) = cache_weak.upgrade() {
                        cache.put(vm);
                    }
                }
                Err(err) => {
                    _ = tx.send(Err(err)).await;
                }
            };
        });

        let data = match rx.recv().await {
            Some(Ok((data, true))) => {
                exec.await?;
                return Ok(ExecResult::Response(data));
            }
            Some(Ok((data, false))) => data,
            Some(Err(err)) => {
                return Ok(ExecResult::Error(err));
            }
            None => {
                return Err(anyhow!("unexpected error"));
            }
        };

        let stream = exec.wait_on_stream(
            tokio_stream::once(Ok((data, false)))
                .chain(ReceiverStream::new(rx))
                .map(|v| match v {
                    Ok((data, true)) => {
                        ExecStreamItem::End(if data.is_empty() { None } else { Some(data) })
                    }
                    Ok((data, false)) => ExecStreamItem::Data(data),
                    Err(err) => ExecStreamItem::Error(err),
                }),
        );

        Ok(ExecResult::Stream(Box::pin(Box::new(stream))))
    }

    pub async fn exec(
        &self,
        script: &str,
        func: String,
        args: Vec<String>,
    ) -> anyhow::Result<ExecResult> {
        let mut hasher = Sha256::new();
        hasher.update(script);
        let hash: [u8; 32] = hasher.finalize().into();

        let vm = self.cache.get(hash);
        let vm = if let Some(vm) = vm {
            vm
        } else {
            let mut vm = self.create(hash).await?;
            vm.python
                .vm()
                .call_eval_script(&mut vm.store, script)
                .await?
                .map_err(|e| anyhow!(e))?;
            vm
        };

        self.exec_impl(func, args, vm).await
    }
}

impl Drop for VmManager {
    fn drop(&mut self) {
        // yield one last time
        self.engine.increment_epoch();
    }
}

struct ChildTask<T>(tokio::task::JoinHandle<T>);

impl<T> ChildTask<T>
where
    T: Send + 'static,
{
    pub fn spawn(f: impl Future<Output = T> + Send + 'static) -> Self {
        Self(tokio::spawn(f))
    }

    pub fn wait_on_stream<U, S: Stream<Item = U> + Unpin>(self, s: S) -> impl Stream<Item = U> {
        ChildStream {
            stream: s,
            _task: self,
        }
    }
}

impl<T> Drop for ChildTask<T> {
    fn drop(&mut self) {
        self.0.abort()
    }
}

impl<T> Future for ChildTask<T> {
    type Output = <tokio::task::JoinHandle<T> as Future>::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

pub struct ChildStream<S: Stream<Item = T> + Unpin, T, U> {
    stream: S,
    _task: ChildTask<U>,
}

impl<S: Stream<Item = T> + Unpin, T, U> Stream for ChildStream<S, T, U> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = Pin::new(&mut self.stream);
        stream.poll_next(cx)
    }
}
