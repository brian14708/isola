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
use component_init::Invoker;
use futures_util::FutureExt;
use pin_project_lite::pin_project;
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use tokio::{io::AsyncWriteExt, sync::mpsc, task::JoinHandle};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tracing::{info, level_filters::LevelFilter};
use wasmtime::{
    component::{Component, Instance},
    Config, Engine, Store,
};

use crate::{
    error::{Error, Result},
    vm::{
        exports::{Argument, Value},
        SandboxPre, Vm, VmState,
    },
    vm_cache::VmCache,
    wasm::logging::bindings::logging::Level,
    Env,
};

const MAX_MEMORY: usize = 64 * 1024 * 1024;
const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct VmManager<E> {
    engine: Engine,
    instance_pre: SandboxPre<VmState<E>>,
    cache: Arc<VmCache<E>>,
    epoch_ticker: JoinHandle<()>,
    _env: std::marker::PhantomData<E>,
}

pub enum ExecStreamItem {
    Data(Vec<u8>),
    End(Option<Vec<u8>>),
    Error(Error),
}

pub enum ExecArgumentValue {
    Cbor(Vec<u8>),
    CborStream(mpsc::Receiver<Vec<u8>>),
}

pub struct ExecArgument {
    pub name: Option<String>,
    pub value: ExecArgumentValue,
}

pub enum ExecSource<'a> {
    Script(&'a str, &'a str),
    Bundle(&'a [u8]),
}

#[derive(serde::Deserialize)]
struct Manifest {
    entrypoint: String,
}

impl<E> VmManager<E> {
    fn cfg() -> Config {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .epoch_interruption(true)
            .cranelift_opt_level(wasmtime::OptLevel::Speed);

        config
    }

    pub async fn compile(path: &Path) -> anyhow::Result<()> {
        let data = std::fs::read(path)?;
        let data = component_init::initialize(&data, |instrumented| {
            async move {
                let mut config = Self::cfg();
                config
                    .epoch_interruption(false)
                    .cranelift_opt_level(wasmtime::OptLevel::None);
                let engine = Engine::new(&config)?;
                let workdir = tempfile::TempDir::with_prefix("vm").map_err(anyhow::Error::from)?;
                let component = Component::new(&engine, &instrumented)?;
                let linker = VmState::new_linker(&engine)?;
                let mut store = VmState::new(&engine, workdir.path(), MAX_MEMORY, MockEnv {});

                let pre = linker.instantiate_pre(&component)?;
                let binding = pre.instantiate_async(&mut store).await?;
                let (_, idx) = component
                    .export_index(None, "promptkit:script/guest")
                    .ok_or_else(|| anyhow!("missing promptkit:script/guest"))?;
                let (_, idx) = component
                    .export_index(Some(&idx), "initialize")
                    .ok_or_else(|| anyhow!("missing promptkit:script/guest.initialize"))?;
                let func = binding
                    .get_typed_func::<(bool,), ()>(&mut store, idx)
                    .map_err(anyhow::Error::from)?;
                func.call_async(&mut store, (true,)).await?;
                func.post_return_async(&mut store).await?;

                Ok(Box::new(MyInvoker {
                    store,
                    instance: binding,
                }) as Box<dyn Invoker>)
            }
            .boxed()
        })
        .await?;

        let config = Self::cfg();
        let engine = Engine::new(&config)?;
        let component = Component::new(&engine, &data)?;
        println!("Serializing...");
        let data = component.serialize()?;
        std::fs::write(path.with_extension("wasm.cache"), data)?;
        Ok(())
    }
}

impl<E> VmManager<E>
where
    E: Env + Send + Sync + Clone,
{
    pub fn new(path: &Path) -> Result<Self> {
        let config = Self::cfg();
        let engine = Engine::new(&config)?;

        info!("Loading module...");
        let component = {
            let cache_path = path.with_extension("wasm.cache");

            let mod_time = std::fs::metadata(path)
                .map_err(anyhow::Error::from)?
                .modified()
                .map_err(anyhow::Error::from)?;
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
                    std::fs::write(cache_path, data).map_err(anyhow::Error::from)?;
                }
                component
            }
        };

        let linker = VmState::new_linker(&engine)?;
        let instance_pre = linker.instantiate_pre(&component)?;

        info!("Loaded module!");

        Ok(Self {
            engine: engine.clone(),
            instance_pre: SandboxPre::new(instance_pre)?,
            cache: Arc::new(VmCache::new()),
            epoch_ticker: tokio::task::spawn(async move {
                let mut interval = tokio::time::interval(EPOCH_TICK);
                loop {
                    interval.tick().await;
                    engine.increment_epoch();
                }
            }),
            _env: std::marker::PhantomData,
        })
    }

    pub async fn create(&self, hash: [u8; 32], env: E) -> Result<Vm<E>> {
        let workdir = tempfile::TempDir::with_prefix("vm").map_err(anyhow::Error::from)?;
        let mut store = VmState::new(&self.engine, workdir.path(), MAX_MEMORY, env);
        store.epoch_deadline_async_yield_and_update(1);

        let bindings = self.instance_pre.instantiate_async(&mut store).await?;
        bindings
            .promptkit_script_guest()
            .call_initialize(&mut store, false)
            .await?;
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
        vm: Vm<E>,
        level: LevelFilter,
    ) -> impl Stream<Item = ExecStreamItem> + Send {
        let (tx, rx) = mpsc::channel(4);
        let cache = self.cache.clone();

        let mut run = vm.run(tx.clone());
        let func = func.to_string();
        let exec = Box::pin(async move {
            let ret = run
                .exec(|vm, mut store| async move {
                    let _ = vm
                        .call_set_log_level(
                            &mut store,
                            match level {
                                LevelFilter::OFF => None,
                                LevelFilter::ERROR => Some(Level::Error),
                                LevelFilter::WARN => Some(Level::Warn),
                                LevelFilter::INFO => Some(Level::Info),
                                LevelFilter::DEBUG | LevelFilter::TRACE => Some(Level::Debug),
                            },
                        )
                        .await;
                    vm.call_call_func(&mut store, &func, &args).await
                })
                .await;
            match ret {
                Ok(Ok(e)) => {
                    let _ = tx.send(ExecStreamItem::End(e)).await;
                    cache.put(run.reuse());
                }
                Ok(Err(err)) => {
                    _ = tx.send(ExecStreamItem::Error(err.into())).await;
                }
                Err(err) => {
                    _ = tx.send(ExecStreamItem::Error(err.into())).await;
                }
            };
        });

        join_with(ReceiverStream::new(rx), exec)
    }

    pub async fn exec(
        &'_ self,
        script: ExecSource<'_>,
        func: &str,
        args: impl IntoIterator<Item = ExecArgument>,
        env: &E,
        level: LevelFilter,
    ) -> Result<impl Stream<Item = ExecStreamItem> + Send> {
        let mut hasher = Sha256::new();
        match script {
            ExecSource::Script(p, s) => {
                hasher.update(p);
                hasher.update(s);
            }
            ExecSource::Bundle(b) => hasher.update(b),
        }
        env.hash(|data| hasher.update(data));
        let hash = hasher.finalize().into();

        let vm = self.cache.get(hash);
        let mut vm = if let Some(vm) = vm {
            vm
        } else {
            let mut vm = self.create(hash, env.clone()).await?;
            match script {
                ExecSource::Script(prelude, script) => {
                    if !prelude.is_empty() {
                        vm.sandbox
                            .promptkit_script_guest()
                            .call_eval_script(&mut vm.store, prelude)
                            .await??;
                    }
                    vm.sandbox
                        .promptkit_script_guest()
                        .call_eval_script(&mut vm.store, script)
                        .await??;
                }
                ExecSource::Bundle(bundle) => {
                    let mut zip =
                        zip::ZipArchive::new(Cursor::new(bundle)).map_err(anyhow::Error::from)?;
                    let manifest: Manifest = serde_json::from_reader(
                        zip.by_name("manifest.json").map_err(anyhow::Error::from)?,
                    )
                    .map_err(anyhow::Error::from)?;

                    let name = hex::encode(hash) + ".zip";
                    let mut file = tokio::fs::File::create(vm.workdir.path().join(&name))
                        .await
                        .map_err(anyhow::Error::from)?;
                    file.write_all(bundle).await.map_err(anyhow::Error::from)?;
                    drop(file);

                    vm.sandbox
                        .promptkit_script_guest()
                        .call_eval_bundle(
                            &mut vm.store,
                            &(String::from("/workdir/") + &name),
                            &manifest.entrypoint,
                        )
                        .await??;
                }
            }
            vm
        };

        Ok(self.exec_impl(
            func,
            args.into_iter()
                .map(|a| {
                    Ok(Argument {
                        name: a.name,
                        value: match a.value {
                            ExecArgumentValue::Cbor(a) => Value::Cbor(a),
                            ExecArgumentValue::CborStream(s) => Value::Iterator(
                                vm.new_iter(Box::pin(ReceiverStream::new(s).map(Value::Cbor)))
                                    .map_err(anyhow::Error::from)?,
                            ),
                        },
                    })
                })
                .collect::<anyhow::Result<_>>()?,
            vm,
            level,
        ))
    }
}

impl<E> Drop for VmManager<E> {
    fn drop(&mut self) {
        self.epoch_ticker.abort();
        // yield one last time
        self.engine.increment_epoch();
    }
}

fn join_with<T>(
    stream: impl Stream<Item = T>,
    task: impl Future<Output = ()>,
) -> impl Stream<Item = T> {
    StreamJoin {
        stream: Some(stream),
        task: Some(task),
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

struct MyInvoker<S> {
    store: Store<VmState<S>>,
    instance: Instance,
}

#[async_trait::async_trait]
impl<S: Send> Invoker for MyInvoker<S> {
    async fn call_s32(&mut self, function: &str) -> anyhow::Result<i32> {
        let func = self
            .instance
            .get_typed_func::<(), (i32,)>(&mut self.store, function)?;
        let result = func.call_async(&mut self.store, ()).await?.0;
        func.post_return_async(&mut self.store).await?;
        Ok(result)
    }

    async fn call_s64(&mut self, function: &str) -> anyhow::Result<i64> {
        let func = self
            .instance
            .get_typed_func::<(), (i64,)>(&mut self.store, function)?;
        let result = func.call_async(&mut self.store, ()).await?.0;
        func.post_return_async(&mut self.store).await?;
        Ok(result)
    }

    async fn call_f32(&mut self, function: &str) -> anyhow::Result<f32> {
        let func = self
            .instance
            .get_typed_func::<(), (f32,)>(&mut self.store, function)?;
        let result = func.call_async(&mut self.store, ()).await?.0;
        func.post_return_async(&mut self.store).await?;
        Ok(result)
    }

    async fn call_f64(&mut self, function: &str) -> anyhow::Result<f64> {
        let func = self
            .instance
            .get_typed_func::<(), (f64,)>(&mut self.store, function)?;
        let result = func.call_async(&mut self.store, ()).await?.0;
        func.post_return_async(&mut self.store).await?;
        Ok(result)
    }

    async fn call_list_u8(&mut self, function: &str) -> anyhow::Result<Vec<u8>> {
        let func = self
            .instance
            .get_typed_func::<(), (Vec<u8>,)>(&mut self.store, function)?;
        let result = func.call_async(&mut self.store, ()).await?.0;
        func.post_return_async(&mut self.store).await?;
        Ok(result)
    }
}

struct MockEnv {}

impl Env for MockEnv {
    type Error = anyhow::Error;

    fn hash(&self, _update: impl FnMut(&[u8])) {}

    #[allow(clippy::manual_async_fn)]
    fn send_request_http<B>(
        &self,
        _request: http::Request<B>,
    ) -> impl Future<
        Output = Result<
            http::Response<
                Pin<
                    Box<
                        dyn futures_core::Stream<
                                Item = Result<http_body::Frame<bytes::Bytes>, Self::Error>,
                            > + Send
                            + Sync
                            + 'static,
                    >,
                >,
            >,
            Self::Error,
        >,
    > + Send
           + 'static
    where
        B: http_body::Body + Send + Sync + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        async { todo!() }
    }
}
