use std::{
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use anyhow::anyhow;
use axum::{
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    Json,
};

use serde_json::{json, value::RawValue};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tracing::info;
use wasmtime::{
    component::{Component, InstancePre, Linker},
    Config, Engine,
};

use crate::{
    vm::{host, http_client, PythonVm, Vm, VmState},
    vm_cache::VmCache,
};

const MAX_MEMORY: usize = 64 * 1024 * 1024;
const EPOCH_TICK: Duration = Duration::from_millis(10);

#[derive(Clone)]
pub struct VmManager {
    inner: Arc<VmManagerInner>,
}

struct VmManagerInner {
    engine: Engine,
    instance_pre: InstancePre<VmState>,
    cache: VmCache,
    _epoch_ticker: ChildTask<()>,
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

        let mut linker = Linker::<VmState>::new(&engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;
        host::add_to_linker(&mut linker, |v: &mut VmState| v)?;
        http_client::add_to_linker(&mut linker, |v: &mut VmState| v)?;
        let instance_pre = linker.instantiate_pre(&component)?;

        info!("Loaded module!");

        Ok(Self {
            inner: Arc::new(VmManagerInner {
                engine: engine.clone(),
                instance_pre,
                cache: VmCache::new(),
                _epoch_ticker: ChildTask::spawn(async move {
                    let mut interval = tokio::time::interval(EPOCH_TICK);
                    loop {
                        interval.tick().await;
                        engine.increment_epoch();
                    }
                }),
            }),
        })
    }

    pub async fn create(&self, hash: [u8; 32]) -> anyhow::Result<Vm> {
        let inner = self.inner.as_ref();
        let mut store = VmState::new(&inner.engine, MAX_MEMORY);
        store.epoch_deadline_async_yield_and_update(1);

        let (bindings, _) = PythonVm::instantiate_pre(&mut store, &inner.instance_pre).await?;
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
    ) -> anyhow::Result<Response> {
        let (tx, mut rx) = mpsc::channel(4);
        vm.store.data_mut().initialize(tx.clone());

        let mgr_weak = Arc::downgrade(&self.inner);
        let exec = ChildTask::spawn(async move {
            let ret = vm
                .python
                .python_vm()
                .call_call_func(&mut vm.store, &func, &args)
                .await
                .and_then(|v| v.map_err(|e| anyhow!(e)));
            match ret {
                Ok(()) => {
                    if let Some(mgr) = mgr_weak.upgrade() {
                        mgr.cache.put(vm);
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
                return match rx.recv().await {
                    Some(Ok(_)) => Err(anyhow!("unexpected")),
                    Some(Err(err)) => Ok((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "error": err.to_string(),
                        })),
                    )
                        .into_response()),
                    None => {
                        Ok((StatusCode::OK, Json(RawValue::from_string(data)?)).into_response())
                    }
                };
            }
            Some(Ok((data, false))) => data,
            Some(Err(err)) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": err.to_string(),
                    })),
                )
                    .into_response())
            }
            None => {
                return Err(anyhow!("unexpected error"));
            }
        };

        let stream = exec.wait_on_stream(
            tokio_stream::once(Ok((data, false)))
                .chain(ReceiverStream::new(rx))
                .map::<anyhow::Result<Event>, _>(|v| match v {
                    Ok((data, true)) => {
                        Ok(Event::default().data(if data.is_empty() { "[DONE]" } else { &data }))
                    }
                    Ok((data, false)) => Ok(Event::default().data(data)),
                    Err(err) => Ok(Event::default()
                        .event("error")
                        .json_data(json!({
                            "error": err.to_string(),
                        }))
                        .unwrap()),
                }),
        );

        Ok(Sse::new(stream)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(1))
                    .text("keepalive"),
            )
            .into_response())
    }

    pub async fn exec(
        &self,
        script: &str,
        func: String,
        args: Vec<String>,
    ) -> anyhow::Result<Response> {
        let mut hasher = Sha256::new();
        hasher.update(script);
        let hash: [u8; 32] = hasher.finalize().into();

        let vm = self.inner.cache.get(hash);
        if let Some(vm) = vm {
            return self.exec_impl(func, args, vm).await;
        }

        let mut vm = self.create(hash).await?;
        vm.python
            .python_vm()
            .call_eval_script(&mut vm.store, script)
            .await?
            .map_err(|e| anyhow!(e))?;

        self.exec_impl(func, args, vm).await
    }
}

impl Drop for VmManagerInner {
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
