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
use component_init_transform::Invoker;
use futures_util::FutureExt;
use pin_project_lite::pin_project;
use rc_zip_tokio::{ReadZip, rc_zip::parse::EntryKind};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::{Stream, StreamExt, wrappers::ReceiverStream};
use tracing::{info, level_filters::LevelFilter};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Instance},
};

use crate::{
    Env,
    env::{RpcConnect, RpcPayload},
    error::{Error, Result},
    vm::{
        OutputCallback, SandboxPre, Vm, VmState,
        exports::{Argument, Value},
    },
    vm_cache::VmCache,
    wasm::logging::bindings::logging::Level,
};

const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct VmManager<E: 'static> {
    engine: Engine,
    instance_pre: SandboxPre<VmState<E>>,
    cache: Arc<VmCache<E>>,
    epoch_ticker: JoinHandle<()>,
    base_dir: PathBuf,
    _env: std::marker::PhantomData<E>,
}

pub enum ExecStreamItem {
    Data(Vec<u8>),
    End(Option<Vec<u8>>),
    Error(Error),
}

pub struct MpscOutputCallback {
    sender: mpsc::Sender<ExecStreamItem>,
}

impl MpscOutputCallback {
    #[must_use]
    pub fn new(sender: mpsc::Sender<ExecStreamItem>) -> Self {
        Self { sender }
    }
}

impl OutputCallback for MpscOutputCallback {
    fn on_result(
        &mut self,
        item: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send>> {
        let sender = self.sender.clone();
        Box::pin(async move {
            sender
                .send(ExecStreamItem::Data(item))
                .await
                .map_err(|e| anyhow!("Send error: {}", e))
        })
    }
}

pub enum ExecArgumentValue {
    Cbor(Vec<u8>),
    CborStream(mpsc::Receiver<Vec<u8>>),
}

pub struct ExecArgument {
    pub name: Option<String>,
    pub value: ExecArgumentValue,
}

pub enum ExecSource {
    Script(String, String),
    Bundle(Vec<u8>),
}

#[derive(serde::Deserialize)]
struct Manifest {
    entrypoint: String,
    prelude: Option<String>,
}

impl<E> VmManager<E> {
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
                let linker = VmState::new_linker(&engine)?;
                let mut store = VmState::new(
                    &engine,
                    &base_dir,
                    workdir.path(),
                    Self::get_max_memory(),
                    MockEnv {},
                );

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
}

impl<E> VmManager<E>
where
    E: Env + Send + Sync + Clone + 'static,
{
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

    pub async fn create(&self, hash: [u8; 32], env: E) -> Result<Vm<E>> {
        let workdir = tempfile::TempDir::with_prefix("vm").map_err(anyhow::Error::from)?;
        let mut store = VmState::new(
            &self.engine,
            &self.base_dir,
            workdir.path(),
            Self::get_max_memory(),
            env,
        );
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
        func: String,
        args: SmallVec<[Argument; 2]>,
        vm: Vm<E>,
        level: LevelFilter,
    ) -> impl Stream<Item = ExecStreamItem> + Send + use<E> {
        let (tx, rx) = mpsc::channel(4);
        let cache = self.cache.clone();

        let mut run = vm.run(MpscOutputCallback::new(tx.clone()));
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
            }
        });

        join_with(ReceiverStream::new(rx), exec)
    }

    pub async fn exec(
        &self,
        id: &str,
        script: ExecSource,
        func: String,
        args: Vec<ExecArgument>,
        env: &E,
        level: LevelFilter,
    ) -> Result<impl Stream<Item = ExecStreamItem> + Send + use<E>> {
        let mut hasher = Sha256::new();
        hasher.update(id);
        match &script {
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
                            .call_eval_script(&mut vm.store, &prelude)
                            .await??;
                    }
                    vm.sandbox
                        .promptkit_script_guest()
                        .call_eval_script(&mut vm.store, &script)
                        .await??;
                }
                ExecSource::Bundle(bundle) => {
                    let base = vm.workdir.path();
                    extract_zip(bundle, base).await?;
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

        Ok(self.exec_impl(
            func,
            args.into_iter()
                .map(|a| {
                    Ok(Argument {
                        name: a.name,
                        value: match a.value {
                            ExecArgumentValue::Cbor(a) => Value::Cbor(a),
                            ExecArgumentValue::CborStream(s) => Value::Iterator(
                                vm.new_iter(ReceiverStream::new(s).map(Value::Cbor))
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

struct MyInvoker<S: 'static> {
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

    #[allow(clippy::manual_async_fn)]
    fn connect_rpc(
        &self,
        _connect: RpcConnect,
        _req: tokio::sync::mpsc::Receiver<RpcPayload>,
        _resp: tokio::sync::mpsc::Sender<anyhow::Result<RpcPayload>>,
    ) -> impl Future<Output = Result<JoinHandle<anyhow::Result<()>>, Self::Error>> + Send + 'static
    {
        async { todo!() }
    }
}

async fn extract_zip(data: impl Into<Vec<u8>>, dest: &Path) -> anyhow::Result<()> {
    let data = data.into();
    let zip = data.read_zip().await?;
    let mut dirs = std::collections::HashSet::new();
    for entry in zip.entries() {
        let outpath = match entry.sanitized_name() {
            Some(v) => dest.join(v),
            None => continue,
        };
        match entry.kind() {
            EntryKind::Directory => {
                if !dirs.contains(&outpath) {
                    tokio::fs::create_dir_all(&outpath).await?;
                    dirs.insert(outpath);
                }
            }
            EntryKind::File => {
                if let Some(p) = outpath.parent() {
                    if !dirs.contains(p) {
                        tokio::fs::create_dir_all(&p).await?;
                        dirs.insert(p.to_owned());
                    }
                }
                let mut out = tokio::fs::File::create(&outpath).await?;
                tokio::io::copy(&mut entry.reader(), &mut out).await?;
            }
            EntryKind::Symlink => {} // skip for now
        }
    }
    Ok(())
}
