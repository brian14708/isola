use std::{
    future::Future,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use anyhow::anyhow;
use bytes::Bytes;
use component_init_transform::Invoker;
use futures::FutureExt;
use pin_project_lite::pin_project;
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tracing::{info, level_filters::LevelFilter};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Instance},
};

use crate::{
    Argument, Source, StreamItem,
    env::{BoxedStream, Env, EnvHandle, EnvHttp, WebsocketMessage},
    error::Result,
    types::ArgumentOwned,
    vm::{OutputCallback, SandboxPre, Vm, VmState},
    vm_cache::VmCache,
    wasm::logging::bindings::logging::Level,
};

const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct VmManager<E: EnvHandle> {
    engine: Engine,
    instance_pre: SandboxPre<VmState<E>>,
    cache: Arc<VmCache<E>>,
    epoch_ticker: JoinHandle<()>,
    base_dir: PathBuf,
    _env: std::marker::PhantomData<E>,
}

pub struct MpscOutputCallback {
    sender: mpsc::Sender<StreamItem>,
}

impl MpscOutputCallback {
    #[must_use]
    pub fn new(sender: mpsc::Sender<StreamItem>) -> Self {
        Self { sender }
    }
}

impl OutputCallback for MpscOutputCallback {
    async fn on_result(&mut self, item: Bytes) -> Result<(), anyhow::Error> {
        let sender = self.sender.clone();
        sender
            .send(StreamItem::Data(item))
            .await
            .map_err(|e| anyhow!("Send error: {}", e))
    }

    async fn on_end(&mut self, item: Bytes) -> Result<(), anyhow::Error> {
        let sender = self.sender.clone();
        sender
            .send(StreamItem::End(if item.is_empty() {
                None
            } else {
                Some(item)
            }))
            .await
            .map_err(|e| anyhow!("Send error: {}", e))
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    entrypoint: String,
    prelude: Option<String>,
}

impl<E: EnvHandle> VmManager<E> {
    fn cfg() -> (Config, String) {
        let mut hash = String::new();
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .epoch_interruption(true)
            .table_lazy_init(false)
            .generate_address_map(false)
            .wasm_backtrace(false)
            .native_unwind_info(false)
            .cranelift_opt_level(wasmtime::OptLevel::Speed);

        if std::env::var("DISABLE_EPOCH_INTERRUPTION")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            config.epoch_interruption(false);
            hash.push_str("no_epoch_interruption");
        }

        (config, hash)
    }

    fn get_max_memory() -> usize {
        std::env::var("VM_MAX_MEMORY")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or(64 * 1024 * 1024)
    }

    fn base_dir(path: &Path) -> PathBuf {
        let mut base_dir = path.to_owned();
        base_dir.pop();
        base_dir.push("wasm32-wasip1");
        base_dir.push("wasi-deps");
        base_dir.push("usr");
        base_dir
    }

    /// Compiles a WASM component from the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the component cannot be read, compiled, or serialized.
    pub async fn compile(path: &Path) -> anyhow::Result<PathBuf> {
        let data = std::fs::read(path)?;
        let base_dir = Self::base_dir(path);
        let data = component_init_transform::initialize(&data, |instrumented| {
            async move {
                let (mut config, _) = Self::cfg();
                config
                    .epoch_interruption(false)
                    .cranelift_opt_level(wasmtime::OptLevel::None);
                let engine = Engine::new(&config)?;
                let workdir = tempfile::TempDir::with_prefix("vm").map_err(anyhow::Error::from)?;
                let component = Component::new(&engine, &instrumented)?;
                let linker = VmState::<CompileEnv>::new_linker(&engine)?;
                let mut store =
                    VmState::new(&engine, &base_dir, workdir.path(), Self::get_max_memory())?;

                let pre = linker.instantiate_pre(&component)?;
                let binding = pre.instantiate_async(&mut store).await?;
                let idx = component
                    .get_export_index(None, "promptkit:script/guest")
                    .ok_or_else(|| anyhow!("missing promptkit:script/guest"))?;
                let idx = component
                    .get_export_index(Some(&idx), "initialize")
                    .ok_or_else(|| anyhow!("missing promptkit:script/guest.initialize"))?;
                let func = binding.get_typed_func::<(bool,), ()>(&mut store, idx)?;
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

        let (config, hash) = Self::cfg();
        let engine = Engine::new(&config)?;
        let component = Component::new(&engine, &data)?;
        println!("Serializing...");
        let data = component.serialize()?;
        let exts = Self::ext(&engine, &hash);
        let compiled_path = path.with_extension(&exts[0]);
        std::fs::write(&compiled_path, data)?;
        #[cfg(unix)]
        {
            if let Some(name) = compiled_path.file_name() {
                for ext in &exts[1..] {
                    let symlink_path = path.with_extension(ext);
                    _ = std::fs::remove_file(&symlink_path);
                    _ = std::os::unix::fs::symlink(name, &symlink_path);
                }
            }
        }
        Ok(compiled_path)
    }

    fn ext(engine: &Engine, feature: &str) -> Vec<String> {
        vec![
            {
                let mut hasher = DefaultHasher::new();
                engine.precompile_compatibility_hash().hash(&mut hasher);
                feature.hash(&mut hasher);
                format!("{:x}.cwasm", hasher.finish())
            },
            {
                let mut hasher = DefaultHasher::new();
                feature.hash(&mut hasher);
                format!("{:x}.cwasm", hasher.finish())
            },
        ]
    }

    /// Creates a new VM manager from the compiled component at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the component cannot be loaded or if the linker fails to initialize.
    pub async fn new(path: &Path) -> Result<Self> {
        let (config, feature_hash) = Self::cfg();
        let engine = Engine::new(&config)?;

        info!("Loading module...");
        let component = (async {
            let mod_time = std::fs::metadata(path)
                .map_err(anyhow::Error::from)?
                .modified()
                .map_err(anyhow::Error::from)?;
            for cache_path in Self::ext(&engine, &feature_hash)
                .into_iter()
                .map(|v| path.with_extension(v))
            {
                let cache = std::fs::metadata(&cache_path)
                    .map_err(anyhow::Error::from)
                    .and_then(|v| {
                        if mod_time <= v.modified()? {
                            Ok::<_, anyhow::Error>(unsafe {
                                Component::deserialize_file(&engine, cache_path)?
                            })
                        } else {
                            Err(anyhow!("cache is outdated"))
                        }
                    });

                if let Ok(c) = cache {
                    return Ok(c);
                }
            }
            let cache_path = Self::compile(path).await?;
            unsafe { Component::deserialize_file(&engine, &cache_path) }
        })
        .await?;

        let linker = VmState::new_linker(&engine)?;
        let instance_pre = linker.instantiate_pre(&component)?;
        Engine::tls_eager_initialize();

        info!("Loaded module!");

        let base_dir = Self::base_dir(path);
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
            base_dir,
            _env: std::marker::PhantomData,
        })
    }

    /// Creates a new VM instance with the given hash and environment.
    ///
    /// # Errors
    ///
    /// Returns an error if the VM cannot be instantiated or initialized.
    pub async fn create(&self, hash: [u8; 32]) -> Result<Vm<E>> {
        let workdir = tempfile::TempDir::with_prefix("vm").map_err(anyhow::Error::from)?;
        let mut store = VmState::new(
            &self.engine,
            &self.base_dir,
            workdir.path(),
            Self::get_max_memory(),
        )?;
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
}

impl<E> VmManager<E>
where
    E: EnvHandle + Env<Callback = MpscOutputCallback>,
{
    fn exec_impl(
        &self,
        func: String,
        mut args: SmallVec<[ArgumentOwned; 2]>,
        vm: Vm<E>,
        env: E,
        level: LevelFilter,
    ) -> impl Stream<Item = StreamItem> + Send + use<E> {
        let (tx, rx) = mpsc::channel(4);
        let cache = self.cache.clone();

        let mut run = vm.run(env, MpscOutputCallback::new(tx.clone()));
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
                    let new_args = args
                        .iter_mut()
                        .map(|a| a.as_value())
                        .collect::<SmallVec<[_; 2]>>();
                    vm.call_call_func(&mut store, &func, &new_args).await
                })
                .await;
            match ret {
                Ok(Ok(())) => {
                    cache.put(run.reuse());
                }
                Ok(Err(err)) => {
                    _ = tx.send(StreamItem::Error(err.into())).await;
                }
                Err(err) => {
                    _ = tx.send(StreamItem::Error(err.into())).await;
                }
            }
        });

        join_with(ReceiverStream::new(rx), exec)
    }

    /// Executes a function in a VM with the given script and arguments.
    ///
    /// # Errors
    ///
    /// Returns an error if the VM cannot be created, the script cannot be evaluated,
    /// or the function arguments cannot be processed.
    pub async fn exec(
        &self,
        id: &str,
        script: Source,
        func: String,
        args: Vec<Argument>,
        env: E,
        level: LevelFilter,
    ) -> Result<impl Stream<Item = StreamItem> + Send + use<E>> {
        let mut hasher = Sha256::new();
        hasher.update(id);
        match &script {
            Source::Script { prelude, code } => {
                hasher.update(prelude);
                hasher.update(code);
            }
            Source::Bundle(b) => hasher.update(b),
        }
        let hash = hasher.finalize().into();

        let vm = self.cache.get(hash);
        let mut vm = if let Some(vm) = vm {
            vm
        } else {
            let mut vm = self.create(hash).await?;
            match script {
                Source::Script { prelude, code } => {
                    if !prelude.is_empty() {
                        vm.sandbox
                            .promptkit_script_guest()
                            .call_eval_script(&mut vm.store, &prelude)
                            .await??;
                    }
                    vm.sandbox
                        .promptkit_script_guest()
                        .call_eval_script(&mut vm.store, &code)
                        .await??;
                }
                Source::Bundle(bundle) => {
                    let base = vm.workdir.path();
                    Source::extract_zip(bundle, base).await?;
                    let manifest: Manifest = serde_json::from_str(
                        &tokio::fs::read_to_string(base.join("manifest.json"))
                            .await
                            .map_err(anyhow::Error::from)?,
                    )
                    .map_err(anyhow::Error::from)?;

                    if let Some(prelude) = &manifest.prelude {
                        vm.sandbox
                            .promptkit_script_guest()
                            .call_eval_script(&mut vm.store, prelude)
                            .await??;
                    }

                    vm.sandbox
                        .promptkit_script_guest()
                        .call_eval_file(
                            &mut vm.store,
                            &["/workdir/", &manifest.entrypoint].join(""),
                        )
                        .await??;
                }
            }
            vm
        };

        let args = args
            .into_iter()
            .map(|a| a.into_owned(&mut vm))
            .collect::<anyhow::Result<_>>()?;

        Ok(self.exec_impl(func, args, vm, env.clone(), level))
    }
}

impl<E: EnvHandle> Drop for VmManager<E> {
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

        if let Some(task) = this.task.as_mut().as_pin_mut()
            && task.poll(cx) == Poll::Ready(())
        {
            this.task.set(None);
        }

        if this.stream.is_none() && this.task.is_none() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

struct MyInvoker<S: EnvHandle> {
    store: Store<VmState<S>>,
    instance: Instance,
}

#[async_trait::async_trait]
impl<S: EnvHandle> Invoker for MyInvoker<S> {
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

#[derive(Clone)]
struct CompileEnv {}

impl Env for CompileEnv {
    type Callback = Self;
    type Error = anyhow::Error;

    async fn hostcall(&self, call_type: &str, payload: &[u8]) -> Result<Vec<u8>, Self::Error> {
        match call_type {
            "echo" => {
                // Simple echo - return the payload as-is
                Ok(payload.to_vec())
            }
            _ => Err(anyhow!("unknown")), // Unknown hostcall type
        }
    }
}

impl EnvHttp for CompileEnv {
    type Error = anyhow::Error;

    async fn send_request_http<B>(
        &self,
        _request: http::Request<B>,
    ) -> std::result::Result<
        http::Response<BoxedStream<http_body::Frame<bytes::Bytes>, Self::Error>>,
        Self::Error,
    >
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        anyhow::bail!("unsupported during compilation")
    }

    async fn connect_websocket<B>(
        &self,
        _request: http::Request<B>,
    ) -> Result<http::Response<BoxedStream<WebsocketMessage, Self::Error>>, Self::Error>
    where
        B: futures::Stream<Item = WebsocketMessage> + Sync + Send + 'static,
    {
        anyhow::bail!("unsupported during compilation")
    }
}

impl OutputCallback for CompileEnv {
    async fn on_result(&mut self, _item: Bytes) -> Result<(), anyhow::Error> {
        anyhow::bail!("unsupported during compilation")
    }
    async fn on_end(&mut self, _item: Bytes) -> Result<(), anyhow::Error> {
        anyhow::bail!("unsupported during compilation")
    }
}
