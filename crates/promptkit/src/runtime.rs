use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};

use anyhow::anyhow;
use bytes::Bytes;
use futures::Stream;
use smallvec::SmallVec;
use tokio::task::JoinHandle;
use tracing::info;
use wasmtime::{Config, Engine, Store, component::Component};
use wasmtime_wizer::{WasmtimeWizerComponent, Wizer};

use crate::{
    BoxedStream, Environment, Error, WebsocketMessage,
    error::Result,
    internal::vm::{InstanceState, SandboxPre, exports::GuestIndices},
};
use crate::{
    environment::OutputCallback,
    internal::vm::{
        HostView, Sandbox, ValueIterator,
        exports::{Argument as RawArgument, Value},
    },
};

const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct Runtime<E: Environment> {
    config: RuntimeConfig,
    engine: Engine,
    instance_pre: SandboxPre<InstanceState<E>>,
    epoch_ticker: JoinHandle<()>,
    _env: std::marker::PhantomData<E>,
}

#[derive(Clone, Debug)]
struct RuntimeConfig {
    max_memory: usize,
    wasm_path: PathBuf,
    cache_path: PathBuf,
    library_path: PathBuf,
    compile_prelude: Option<String>,
}

impl<E: Environment> Runtime<E> {
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    fn engine_config() -> Config {
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
        config
    }

    async fn compile(cfg: &RuntimeConfig, out: &Path) -> anyhow::Result<()> {
        let cfg = cfg.clone();
        let path = cfg.wasm_path.clone();
        let cache_path = cfg.cache_path.clone();
        tokio::fs::create_dir_all(&cache_path).await?;
        let lib_path = cfg.library_path.clone();
        let data = std::fs::read(&path)?;
        let out = out.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Runtime::new()?;
            let data = rt.block_on(async {
                let w = Wizer::new();
                let (cx, instrumented_wasm) = w.instrument_component(&data)?;

                let mut config = Self::engine_config();
                config
                    .epoch_interruption(false)
                    .cranelift_opt_level(wasmtime::OptLevel::None);
                let engine = Engine::new(&config)?;
                let mut store = InstanceState::new(
                    &engine,
                    &lib_path,
                    Some(&lib_path),
                    cfg.max_memory,
                    CompileEnv,
                )?;
                let component = Component::new(&engine, &instrumented_wasm)?;
                let linker = InstanceState::<CompileEnv>::new_linker(&engine)?;
                let pre = linker.instantiate_pre(&component)?;
                let instance = pre.instantiate_async(&mut store).await?;
                let guest = GuestIndices::new(&pre)?.load(&mut store, &instance)?;
                guest
                    .call_initialize(
                        &mut store,
                        true,
                        &["/lib/bundle.zip", "/workdir"],
                        cfg.compile_prelude.as_deref(),
                    )
                    .await?;
                w.snapshot_component(
                    cx,
                    &mut WasmtimeWizerComponent {
                        store: &mut store,
                        instance,
                    },
                )
                .await
            })?;

            let config = Self::engine_config();
            let engine = Engine::new(&config)?;
            let component = Component::new(&engine, &data)?;
            let data = component.serialize()?;
            std::fs::write(&out, data)?;
            Ok(())
        })
        .await
        .map_err(|_e| anyhow!("Join error"))?
    }

    /// # Errors
    /// Returns an error if the instance cannot be created.
    pub async fn instantiate(&self, workdir: Option<&Path>, env: E) -> Result<Instance<E>> {
        let mut store = InstanceState::new(
            &self.engine,
            &self.config.library_path,
            workdir,
            self.config.max_memory,
            env,
        )?;
        store.epoch_deadline_async_yield_and_update(1);

        let bindings = self.instance_pre.instantiate_async(&mut store).await?;
        Ok(Instance {
            store,
            sandbox: bindings,
            _env: std::marker::PhantomData,
        })
    }
}

impl<E: Environment> Drop for Runtime<E> {
    fn drop(&mut self) {
        self.epoch_ticker.abort();
        self.engine.increment_epoch();
    }
}

pub struct RuntimeBuilder {
    max_memory: usize,
    cache_path: Option<std::path::PathBuf>,
    library_path: Option<std::path::PathBuf>,
    compile_prelude: Option<String>,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            max_memory: 64 * 1024 * 1024,
            cache_path: None,
            library_path: None,
            compile_prelude: None,
        }
    }
}

impl RuntimeBuilder {
    #[must_use]
    pub const fn max_memory(mut self, bytes: usize) -> Self {
        self.max_memory = bytes;
        self
    }

    #[must_use]
    pub fn cache_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.cache_path = Some(p.into());
        self
    }

    #[must_use]
    pub fn compile_prelude(mut self, s: impl Into<String>) -> Self {
        self.compile_prelude = Some(s.into());
        self
    }

    #[must_use]
    pub fn library_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.library_path = Some(p.into());
        self
    }

    /// # Errors
    /// Returns an error if the runtime cannot be built.
    pub async fn build<E: Environment>(self, wasm: impl AsRef<Path>) -> Result<Runtime<E>> {
        let wasm_path = std::fs::canonicalize(wasm.as_ref()).map_err(|e| Error::Other(e.into()))?;
        let parent = wasm_path
            .parent()
            .ok_or_else(|| Error::Other(anyhow!("Wasm path has no parent directory")))?
            .to_path_buf();

        let config = RuntimeConfig {
            max_memory: self.max_memory,
            wasm_path: wasm_path.clone(),
            cache_path: self.cache_path.unwrap_or_else(|| parent.join("cache")),
            library_path: self.library_path.unwrap_or_else(|| parent.join("lib")),
            compile_prelude: self.compile_prelude,
        };

        let path = &config.wasm_path;
        let engine_config = Runtime::<E>::engine_config();
        let engine = Engine::new(&engine_config)?;

        info!("Loading module...");
        let component = (async {
            let mod_time = std::fs::metadata(path)
                .map_err(anyhow::Error::from)?
                .modified()
                .map_err(anyhow::Error::from)?;
            let cache_path = config.cache_path.join("module.cwasm");
            let cache = std::fs::metadata(&cache_path)
                .map_err(anyhow::Error::from)
                .and_then(|v| {
                    if mod_time <= v.modified()? {
                        Ok::<_, anyhow::Error>(unsafe {
                            Component::deserialize_file(&engine, &cache_path)?
                        })
                    } else {
                        Err(anyhow!("cache is outdated"))
                    }
                });

            if let Ok(c) = cache {
                return Ok(c);
            }
            Runtime::<E>::compile(&config, &cache_path).await?;
            unsafe { Component::deserialize_file(&engine, &cache_path) }
        })
        .await?;

        let linker = InstanceState::new_linker(&engine)?;
        let instance_pre = linker.instantiate_pre(&component)?;
        Engine::tls_eager_initialize();

        info!("Loaded module!");

        Ok(Runtime {
            config,
            engine: engine.clone(),
            instance_pre: SandboxPre::new(instance_pre)?,
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
}

pub struct Instance<E: Environment> {
    store: Store<InstanceState<E>>,
    sandbox: Sandbox,
    _env: std::marker::PhantomData<E>,
}

pub enum Argument {
    Cbor(Bytes),
    CborStream(Pin<Box<dyn Stream<Item = Bytes> + Send>>),
}

impl<E: Environment> Instance<E> {
    /// # Errors
    /// Returns an error if the script evaluation fails.
    pub async fn eval_script(&mut self, code: impl AsRef<str>) -> Result<()> {
        self.sandbox
            .promptkit_script_guest()
            .call_eval_script(&mut self.store, code.as_ref())
            .await??;
        Ok(())
    }

    /// # Errors
    /// Returns an error if the file evaluation fails.
    pub async fn eval_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = Path::new("/workdir").join(path.as_ref());
        self.sandbox
            .promptkit_script_guest()
            .call_eval_file(&mut self.store, &path.to_string_lossy())
            .await??;
        Ok(())
    }

    /// # Errors
    /// Returns an error if the function execution fails.
    pub async fn execute(
        &mut self,
        function: &str,
        mut args: Vec<(Option<String>, Argument)>,
        callback: E::Callback,
    ) -> Result<()> {
        let mut internal_args = SmallVec::<[RawArgument; 2]>::new();

        for (name, arg) in &mut args {
            let arg_owned = match arg {
                Argument::Cbor(data) => RawArgument {
                    name: name.as_deref(),
                    value: Value::Cbor(AsRef::<[u8]>::as_ref(data)),
                },
                Argument::CborStream(_) => {
                    let Argument::CborStream(stream) =
                        std::mem::replace(arg, Argument::Cbor(Bytes::new()))
                    else {
                        unreachable!()
                    };
                    let iter = self
                        .store
                        .data_mut()
                        .table()
                        .push(ValueIterator::new(stream))
                        .map_err(anyhow::Error::from)?;
                    RawArgument {
                        name: name.as_deref(),
                        value: Value::CborIterator(iter),
                    }
                }
            };
            internal_args.push(arg_owned);
        }

        let func = function.to_string();

        let vm_args = internal_args;

        self.store.data_mut().callback = Some(callback);
        let result = self
            .sandbox
            .promptkit_script_guest()
            .call_call_func(&mut self.store, &func, &vm_args)
            .await;
        self.store.data_mut().callback = None;
        Ok(result??)
    }

    /// # Errors
    /// Returns an error if the memory usage cannot be determined.
    pub fn memory_usage(&self) -> Result<usize> {
        Ok(self.store.data().limiter.current())
    }
}

#[derive(Clone)]
struct CompileEnv;

impl Environment for CompileEnv {
    type Error = std::io::Error;
    type Callback = NilCallback;

    async fn hostcall(&self, _call_type: &str, _payload: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Err(std::io::Error::other("unsupported during compilation"))
    }

    async fn http_request<B>(
        &self,
        _request: http::Request<B>,
    ) -> Result<http::Response<BoxedStream<http_body::Frame<bytes::Bytes>, Self::Error>>, Self::Error>
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        Err(std::io::Error::other("unsupported during compilation"))
    }

    async fn websocket_connect<B>(
        &self,
        _request: http::Request<B>,
    ) -> Result<http::Response<BoxedStream<WebsocketMessage, Self::Error>>, Self::Error>
    where
        B: futures::Stream<Item = WebsocketMessage> + Sync + Send + 'static,
    {
        Err(std::io::Error::other("unsupported during compilation"))
    }
}

struct NilCallback;

impl OutputCallback for NilCallback {
    type Error = Infallible;

    async fn on_result(&mut self, _item: Bytes) -> Result<(), Infallible> {
        Ok(())
    }
    async fn on_end(&mut self, _item: Bytes) -> Result<(), Infallible> {
        Ok(())
    }
}
