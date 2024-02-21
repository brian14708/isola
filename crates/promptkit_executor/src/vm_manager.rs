use std::{
    future::Future,
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
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tracing::info;
use wasmtime::{
    component::{Component, InstancePre},
    Config, Engine,
};

use crate::{
    trace::BoxedTracer,
    vm::{exports::Argument, PythonVm, Vm, VmState},
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

pub enum ExecStreamItem {
    Data(String),
    End(Option<String>),
    Error(anyhow::Error),
}

pub type ExecResult = Pin<Box<dyn tokio_stream::Stream<Item = ExecStreamItem> + Send>>;

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

            if let Ok(c) = cache {
                c
            } else {
                let component = Component::from_file(&engine, path)?;
                let data = component.serialize()?;
                std::fs::write(cache_path, data)?;
                component
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
        let mut store = VmState::new(&self.engine, MAX_MEMORY);
        store.epoch_deadline_async_yield_and_update(1);

        let (bindings, _) = PythonVm::instantiate_pre(&mut store, &self.instance_pre).await?;
        Ok(Vm {
            hash,
            store,
            python: bindings,
        })
    }

    fn exec_impl(
        &self,
        func: String,
        args: Vec<String>,
        tracer: Option<BoxedTracer>,
        vm: Vm,
    ) -> ExecResult {
        let (tx, rx) = mpsc::channel(4);
        let cache = self.cache.clone();

        let mut run = vm.run(tracer, tx.clone());
        let exec = Some(Box::pin(async move {
            let args = args
                .into_iter()
                .map(Argument::Json)
                .collect::<SmallVec<[_; 2]>>();
            let ret = run
                .exec(|vm, store| vm.call_call_func(store, &func, &args))
                .await
                .and_then(|v| v.map_err(|e| anyhow!(e)));
            match ret {
                Ok(()) => {
                    cache.put(run.reuse());
                }
                Err(err) => {
                    _ = tx.send(Err(err)).await;
                }
            };
        }));

        let stream = join_with(
            ReceiverStream::new(rx).map(|v| match v {
                Ok((data, true)) => {
                    ExecStreamItem::End(if data.is_empty() { None } else { Some(data) })
                }
                Ok((data, false)) => ExecStreamItem::Data(data),
                Err(err) => ExecStreamItem::Error(err),
            }),
            exec,
        );

        Box::pin(stream)
    }

    pub async fn exec(
        &'_ self,
        script: &str,
        func: String,
        args: Vec<String>,
        tracer: Option<BoxedTracer>,
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

        Ok(self.exec_impl(func, args, tracer, vm))
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
        if let Some(task) = this.task.as_mut().as_pin_mut() {
            if task.poll(cx) == Poll::Ready(()) {
                this.task.set(None);
            }
        }

        if let Some(stream) = this.stream.as_mut().as_pin_mut() {
            match stream.poll_next(cx) {
                Poll::Ready(None) => this.stream.set(None),
                Poll::Ready(Some(v)) => return Poll::Ready(Some(v)),
                Poll::Pending => return Poll::Pending,
            }
        }

        if this.stream.is_none() && this.task.is_none() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}
