use crate::TRACE_TARGET_SCRIPT;
use crate::{
    BoxError, Host, NetworkPolicy, OutputSink, Result,
    error::Error,
    internal::sandbox::{
        InstanceState, SandboxPre,
        exports::GuestIndices,
        exports::{Argument as RawArgument, Value},
    },
    net::{AclPolicyBuilder, AclRule, AclScheme, AllowAllPolicy},
};
use bytes::Bytes;
use component_init_transform::Invoker;
use futures::{FutureExt, Stream};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write as _,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use tracing::{Instrument, info_span, warn};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Instance as WasmInstance},
};

// For `InstanceState::table()` when pushing iterator resources.
use crate::internal::sandbox::HostView as _;

const EPOCH_TICK: Duration = Duration::from_millis(10);
const DEFAULT_MAX_REDIRECTS: usize = 10;
// Wasmtime epoch deadlines are relative deltas (`current_epoch + delta`).
// Using `u64::MAX` can wrap and cause immediate interrupts.
const NO_DEADLINE_TICKS: u64 = u64::MAX / 2;

fn default_network_policy() -> Arc<dyn NetworkPolicy> {
    Arc::new(
        AclPolicyBuilder::new()
            .deny_private_ranges(true)
            .push(AclRule::allow().schemes([
                AclScheme::Http,
                AclScheme::Https,
                AclScheme::Ws,
                AclScheme::Wss,
            ]))
            .build(),
    )
}

#[derive(Clone, Debug)]
pub enum CacheConfig {
    /// Use `<wasm_parent>/cache`.
    Default,
    /// Disable `.cwasm` caching (compile every time).
    Disabled,
    /// Store cached artifacts in this directory.
    Dir(PathBuf),
}

#[derive(Clone, Debug)]
pub struct CompileConfig {
    pub opt_level: wasmtime::OptLevel,
    pub cache: CacheConfig,
    pub max_memory: usize,
}

impl Default for CompileConfig {
    fn default() -> Self {
        Self {
            opt_level: wasmtime::OptLevel::Speed,
            cache: CacheConfig::Default,
            max_memory: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct InitConfig {
    pub preinit: bool,
    pub bundle_paths: Vec<String>,
    pub prelude: Option<String>,
}

impl InitConfig {
    #[must_use]
    pub fn default_python() -> Self {
        Self {
            preinit: true,
            bundle_paths: vec!["/lib/bundle.zip".to_string(), "/workdir".to_string()],
            prelude: Some("import sandbox.asyncio".to_string()),
        }
    }
}

impl Default for InitConfig {
    fn default() -> Self {
        Self {
            preinit: true,
            bundle_paths: Vec::new(),
            prelude: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CallOptions {
    timeout: Option<Duration>,
}

impl CallOptions {
    #[must_use]
    pub const fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    pub(crate) const fn timeout_opt(self) -> Option<Duration> {
        self.timeout
    }
}

pub enum ArgValue {
    Cbor(Bytes),
    CborStream(Pin<Box<dyn Stream<Item = Bytes> + Send>>),
}

pub struct Arg {
    pub name: Option<String>,
    pub value: ArgValue,
}

pub type Args = Vec<Arg>;

impl core::fmt::Debug for ArgValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Cbor(bytes) => f.debug_tuple("Cbor").field(bytes).finish(),
            Self::CborStream(_) => f.debug_tuple("CborStream").field(&"<stream>").finish(),
        }
    }
}

impl core::fmt::Debug for Arg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Arg")
            .field("name", &self.name)
            .field("value", &self.value)
            .finish()
    }
}

#[derive(Clone)]
struct ModuleConfig {
    max_memory: usize,
    wasm_path: PathBuf,
    lib_dir: PathBuf,
    cache_dir: Option<PathBuf>,
    init: InitConfig,
    compile: CompileConfig,
    policy: std::sync::Arc<dyn NetworkPolicy>,
    max_redirects: usize,
}

pub struct ModuleBuilder {
    init: InitConfig,
    compile: CompileConfig,
    lib_dir: Option<PathBuf>,
    policy: std::sync::Arc<dyn NetworkPolicy>,
    max_redirects: usize,
    engine_config: Option<Config>,
}

impl Default for ModuleBuilder {
    fn default() -> Self {
        Self {
            init: InitConfig::default_python(),
            compile: CompileConfig::default(),
            lib_dir: None,
            policy: default_network_policy(),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            engine_config: None,
        }
    }
}

impl ModuleBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn init(mut self, init: InitConfig) -> Self {
        self.init = init;
        self
    }

    #[must_use]
    pub fn compile_config(mut self, cfg: CompileConfig) -> Self {
        self.compile = cfg;
        self
    }

    #[must_use]
    pub fn lib_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.lib_dir = Some(p.into());
        self
    }

    #[must_use]
    pub fn network_policy(mut self, policy: std::sync::Arc<dyn NetworkPolicy>) -> Self {
        self.policy = policy;
        self
    }

    #[must_use]
    pub const fn max_redirects(mut self, max: usize) -> Self {
        self.max_redirects = max;
        self
    }

    #[must_use]
    pub fn engine_config(mut self, cfg: Config) -> Self {
        self.engine_config = Some(cfg);
        self
    }

    /// # Errors
    /// Returns an error if the module cannot be built or compiled.
    pub async fn build<H: Host + Clone>(self, wasm: impl AsRef<Path>) -> Result<Module<H>> {
        let wasm_path = std::fs::canonicalize(wasm.as_ref()).map_err(Error::Io)?;
        let parent = wasm_path
            .parent()
            .ok_or_else(|| Error::Wasm(anyhow::anyhow!("wasm path has no parent directory")))?
            .to_path_buf();

        let lib_dir = self.lib_dir.unwrap_or_else(|| parent.join("lib"));

        let cache_dir = match &self.compile.cache {
            CacheConfig::Default => Some(parent.join("cache")),
            CacheConfig::Disabled => None,
            CacheConfig::Dir(p) => Some(p.clone()),
        };

        let cfg = ModuleConfig {
            max_memory: self.compile.max_memory,
            wasm_path,
            lib_dir,
            cache_dir,
            init: self.init,
            compile: self.compile,
            policy: self.policy,
            max_redirects: self.max_redirects,
        };

        let custom_engine_config = self.engine_config.is_some();
        let mut engine_cfg = self.engine_config.unwrap_or_default();
        if custom_engine_config {
            warn!(
                "custom wasmtime::Config provided; isola will force required fields (component model, async support, epoch interruption)"
            );
        }
        configure_engine(&mut engine_cfg, cfg.compile.opt_level, true);
        let engine = Engine::new(&engine_cfg).map_err(Error::Wasm)?;

        let span = info_span!(target: TRACE_TARGET_SCRIPT, "module.build");
        let component = async { load_or_compile_component(&engine, &cfg).await }
            .instrument(span)
            .await?;

        let linker = InstanceState::<H>::new_linker(&engine).map_err(Error::Wasm)?;
        let pre = linker.instantiate_pre(&component).map_err(Error::Wasm)?;
        Engine::tls_eager_initialize();

        let epoch_engine = engine.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let epoch_ticker = std::thread::Builder::new()
            .name("isola-epoch-ticker".to_string())
            .spawn(move || {
                // Keep epoch progression independent of Tokio scheduling.
                // This avoids timeout starvation in current-thread runtimes.
                loop {
                    if stop_bg.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::park_timeout(EPOCH_TICK);
                    epoch_engine.increment_epoch();
                }
            })
            .map_err(Error::Io)?;

        Ok(Module {
            config: cfg,
            pre: SandboxPre::new(pre).map_err(Error::Wasm)?,
            ticker: Arc::new(EpochTicker {
                handle: Some(epoch_ticker),
                stop,
                engine: engine.clone(),
            }),
            engine,
            _marker: std::marker::PhantomData,
        })
    }
}

/// Shared epoch ticker that keeps incrementing the engine epoch as long as any
/// `Module` or `Sandbox` holds a reference.
struct EpochTicker {
    handle: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    engine: Engine,
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
        // One final increment so any in-flight epoch waits resolve immediately.
        self.engine.increment_epoch();
    }
}

pub struct Module<H: Host + Clone> {
    config: ModuleConfig,
    engine: Engine,
    pre: SandboxPre<InstanceState<H>>,
    ticker: Arc<EpochTicker>,
    _marker: std::marker::PhantomData<H>,
}

pub struct Sandbox<H: Host + Clone> {
    store: Store<InstanceState<H>>,
    bindings: crate::internal::sandbox::Sandbox,
    /// Keeps the epoch ticker alive for the lifetime of this sandbox.
    _ticker: Arc<EpochTicker>,
}

impl<H: Host + Clone> Module<H> {
    /// # Errors
    /// Returns an error if instantiation fails.
    pub async fn instantiate(&self, workdir: Option<&Path>, host: H) -> Result<Sandbox<H>> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.instantiate");
        let ticker = Arc::clone(&self.ticker);
        async move {
            // Host controls guest log verbosity.
            let level = level_filter_to_wasi(host.log_level());

            let mut store = InstanceState::new(
                &self.engine,
                &self.config.lib_dir,
                workdir,
                self.config.max_memory,
                host,
                self.config.policy.clone(),
                self.config.max_redirects,
            )
            .map_err(Error::Wasm)?;

            store.epoch_deadline_trap();
            store.set_epoch_deadline(NO_DEADLINE_TICKS);

            let bindings = self
                .pre
                .instantiate_async(&mut store)
                .await
                .map_err(Error::Wasm)?;

            bindings
                .isola_script_guest()
                .call_set_log_level(&mut store, level)
                .await
                .map_err(Error::Wasm)?;

            Ok(Sandbox {
                store,
                bindings,
                _ticker: ticker,
            })
        }
        .instrument(span)
        .await
    }
}

/// RAII guard that resets the epoch deadline and clears the output sink when
/// dropped, even if the call panics or returns early.
struct CallCleanup<'a, H: Host + Clone> {
    store: &'a mut Store<InstanceState<H>>,
}

impl<H: Host + Clone> Drop for CallCleanup<'_, H> {
    fn drop(&mut self) {
        self.store.data_mut().set_sink(None);
        self.store.set_epoch_deadline(NO_DEADLINE_TICKS);
    }
}

impl<H: Host + Clone> std::ops::Deref for CallCleanup<'_, H> {
    type Target = Store<InstanceState<H>>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl<H: Host + Clone> std::ops::DerefMut for CallCleanup<'_, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl<H: Host + Clone> wasmtime::AsContext for CallCleanup<'_, H> {
    type Data = InstanceState<H>;

    fn as_context(&self) -> wasmtime::StoreContext<'_, Self::Data> {
        wasmtime::AsContext::as_context(&*self.store)
    }
}

impl<H: Host + Clone> wasmtime::AsContextMut for CallCleanup<'_, H> {
    fn as_context_mut(&mut self) -> wasmtime::StoreContextMut<'_, Self::Data> {
        wasmtime::AsContextMut::as_context_mut(&mut *self.store)
    }
}

impl<H: Host + Clone> Sandbox<H> {
    /// # Errors
    /// Returns an error if the script evaluation fails.
    pub async fn eval_script(&mut self, code: impl AsRef<str>) -> Result<()> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.eval_script");
        let code = code.as_ref().to_string();
        self.bindings
            .isola_script_guest()
            .call_eval_script(&mut self.store, &code)
            .instrument(span)
            .await
            .map_err(Error::Wasm)??;
        Ok(())
    }

    /// # Errors
    /// Returns an error if the file evaluation fails.
    pub async fn eval_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.eval_file");
        let path = Path::new("/workdir").join(path.as_ref());
        let path = path.to_string_lossy().to_string();
        self.bindings
            .isola_script_guest()
            .call_eval_file(&mut self.store, &path)
            .instrument(span)
            .await
            .map_err(Error::Wasm)??;
        Ok(())
    }

    /// # Errors
    /// Returns an error if the function execution fails.
    pub async fn call(
        &mut self,
        function: &str,
        mut args: Args,
        sink: impl OutputSink,
        opts: CallOptions,
    ) -> Result<()> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.call");
        async move {
            let mut store = CallCleanup {
                store: &mut self.store,
            };
            let mut internal_args = SmallVec::<[RawArgument; 2]>::new();

            for arg in &mut args {
                let arg_owned = match &mut arg.value {
                    ArgValue::Cbor(data) => RawArgument {
                        name: arg.name.as_deref(),
                        value: Value::Cbor(AsRef::<[u8]>::as_ref(data)),
                    },
                    ArgValue::CborStream(stream) => {
                        let empty: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
                            Box::pin(futures::stream::empty());
                        let stream = std::mem::replace(stream, empty);
                        let iter = store
                            .data_mut()
                            .table()
                            .push(crate::internal::sandbox::ValueIterator::new(stream))
                            .map_err(|e| Error::Wasm(e.into()))?;
                        RawArgument {
                            name: arg.name.as_deref(),
                            value: Value::CborIterator(iter),
                        }
                    }
                };
                internal_args.push(arg_owned);
            }

            let func = function.to_string();

            // Configure per-call epoch deadline.
            if let Some(d) = opts.timeout_opt() {
                let ticks = duration_to_epoch_ticks(d);
                store.set_epoch_deadline(ticks);
            } else {
                store.set_epoch_deadline(NO_DEADLINE_TICKS);
            }

            store.data_mut().set_sink(Some(Box::new(sink)));
            let result = self
                .bindings
                .isola_script_guest()
                .call_call_func(&mut store, &func, &internal_args)
                .await;
            store.data_mut().set_sink(None);

            result.map_err(Error::Wasm)??;
            Ok(())
        }
        .instrument(span)
        .await
    }

    #[must_use]
    pub fn memory_usage(&self) -> usize {
        self.store.data().limiter.current()
    }
}

fn duration_to_epoch_ticks(d: Duration) -> u64 {
    // EPOCH_TICK is 10ms; ensure at least 1 tick for non-zero deadlines.
    let nanos = d.as_nanos();
    if nanos == 0 {
        return 1;
    }
    let tick = EPOCH_TICK.as_nanos();
    let ticks = nanos.div_ceil(tick);
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

const fn level_filter_to_wasi(
    level: tracing::level_filters::LevelFilter,
) -> Option<crate::internal::wasm::logging::bindings::logging::Level> {
    use crate::internal::wasm::logging::bindings::logging::Level as L;
    match level {
        tracing::level_filters::LevelFilter::OFF => None,
        tracing::level_filters::LevelFilter::ERROR => Some(L::Error),
        tracing::level_filters::LevelFilter::WARN => Some(L::Warn),
        tracing::level_filters::LevelFilter::INFO => Some(L::Info),
        tracing::level_filters::LevelFilter::DEBUG => Some(L::Debug),
        tracing::level_filters::LevelFilter::TRACE => Some(L::Trace),
    }
}

fn configure_engine(cfg: &mut Config, opt_level: wasmtime::OptLevel, epoch_interruption: bool) {
    cfg.wasm_component_model(true);
    cfg.async_support(true);
    cfg.epoch_interruption(epoch_interruption);
    cfg.table_lazy_init(false);
    cfg.generate_address_map(false);
    cfg.wasm_backtrace(false);
    cfg.native_unwind_info(false);
    cfg.cranelift_opt_level(opt_level);
}

async fn load_or_compile_component(engine: &Engine, cfg: &ModuleConfig) -> Result<Component> {
    let wasm_bytes = tokio::fs::read(&cfg.wasm_path).await.map_err(Error::Io)?;

    let Some(cache_dir) = &cfg.cache_dir else {
        let bytes = compile_serialized_component(engine, cfg, &wasm_bytes).await?;
        // SAFETY: bytes are produced by wasmtime for the same version/config; if incompatible,
        // deserialization will fail and surface as an error.
        let component = unsafe { Component::deserialize(engine, &bytes) }.map_err(Error::Wasm)?;
        return Ok(component);
    };

    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(Error::Io)?;
    let key = cache_key(engine, cfg, &wasm_bytes).await?;
    let cache_path = cache_dir.join(format!("{key}.cwasm"));

    if let Ok(component) = unsafe { Component::deserialize_file(engine, &cache_path) } {
        return Ok(component);
    }

    let bytes = compile_serialized_component(engine, cfg, &wasm_bytes).await?;
    tokio::fs::write(&cache_path, &bytes)
        .await
        .map_err(Error::Io)?;

    let component =
        unsafe { Component::deserialize_file(engine, &cache_path) }.map_err(Error::Wasm)?;
    Ok(component)
}

fn engine_fingerprint(engine: &Engine) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
}

async fn cache_key(engine: &Engine, cfg: &ModuleConfig, wasm_bytes: &[u8]) -> Result<String> {
    let mut wasm_h = Sha256::new();
    wasm_h.update(wasm_bytes);
    let wasm_digest = wasm_h.finalize();

    let mut h = Sha256::new();
    h.update(b"isola-cache-v2\0");
    h.update(wasm_digest);
    h.update(engine_fingerprint(engine).to_le_bytes());

    h.update([u8::from(cfg.init.preinit)]);
    h.update((cfg.init.bundle_paths.len() as u64).to_le_bytes());
    for p in &cfg.init.bundle_paths {
        h.update(p.as_bytes());
        h.update([0]);
        if let Some(rest) = p.strip_prefix("/lib/") {
            let full = cfg.lib_dir.join(rest);
            let bytes = tokio::fs::read(&full).await.map_err(Error::Io)?;
            let mut fh = Sha256::new();
            fh.update(&bytes);
            let file_digest = fh.finalize();
            h.update(rest.as_bytes());
            h.update([0]);
            h.update(file_digest);
        }
    }

    if let Some(prelude) = &cfg.init.prelude {
        h.update([1]);
        h.update(prelude.as_bytes());
    } else {
        h.update([0]);
    }

    h.update((cfg.compile.max_memory as u64).to_le_bytes());
    h.update(match cfg.compile.opt_level {
        wasmtime::OptLevel::None => &[0u8] as &[u8],
        wasmtime::OptLevel::Speed => &[1],
        wasmtime::OptLevel::SpeedAndSize => &[2],
        _ => &[255],
    });

    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(&mut out, "{b:02x}");
    }
    Ok(out)
}

async fn compile_serialized_component(
    engine: &Engine,
    cfg: &ModuleConfig,
    wasm_bytes: &[u8],
) -> Result<Vec<u8>> {
    let engine = engine.clone();
    let cfg = cfg.clone();
    let wasm_bytes = wasm_bytes.to_vec();

    tokio::task::spawn_blocking(move || {
        // Reuse the current runtime handle. `spawn_blocking` threads are not async
        // contexts, so `block_on` is safe here.
        tokio::runtime::Handle::current().block_on(async move {
            let data = component_init_transform::initialize(&wasm_bytes, |instrumented| {
                let engine = engine.clone();
                async move {
                    let component = Component::new(&engine, &instrumented).map_err(Error::Wasm)?;

                    let linker =
                        InstanceState::<CompileHost>::new_linker(&engine).map_err(Error::Wasm)?;
                    let mut store = InstanceState::new(
                        &engine,
                        &cfg.lib_dir,
                        Some(&cfg.lib_dir),
                        cfg.compile.max_memory,
                        CompileHost,
                        std::sync::Arc::new(AllowAllPolicy),
                        cfg.max_redirects,
                    )
                    .map_err(Error::Wasm)?;
                    store.epoch_deadline_async_yield_and_update(1);

                    let pre = linker.instantiate_pre(&component).map_err(Error::Wasm)?;
                    let binding = pre
                        .instantiate_async(&mut store)
                        .await
                        .map_err(Error::Wasm)?;
                    let guest = GuestIndices::new(&pre)
                        .map_err(Error::Wasm)?
                        .load(&mut store, &binding)
                        .map_err(Error::Wasm)?;

                    let bundle: Vec<&str> =
                        cfg.init.bundle_paths.iter().map(String::as_str).collect();
                    guest
                        .call_initialize(
                            &mut store,
                            cfg.init.preinit,
                            &bundle,
                            cfg.init.prelude.as_deref(),
                        )
                        .await
                        .map_err(Error::Wasm)?;

                    Ok(Box::new(MyInvoker {
                        store,
                        instance: binding,
                    }) as Box<dyn Invoker>)
                }
                .boxed()
            })
            .await
            .map_err(Error::Wasm)?;

            let component = Component::new(&engine, &data).map_err(Error::Wasm)?;
            component.serialize().map_err(Error::Wasm)
        })
    })
    .await
    .map_err(|e| Error::Wasm(anyhow::Error::from(e)))?
}

// Helper structs for compilation pre-init.

struct MyInvoker<S: Host + Clone> {
    store: Store<InstanceState<S>>,
    instance: WasmInstance,
}

#[async_trait::async_trait]
impl<S: Host + Clone> Invoker for MyInvoker<S> {
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
struct CompileHost;

#[async_trait::async_trait]
impl Host for CompileHost {
    async fn hostcall(
        &self,
        _call_type: &str,
        _payload: Bytes,
    ) -> core::result::Result<Bytes, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }

    async fn http_request(
        &self,
        _req: crate::HttpRequest,
    ) -> core::result::Result<crate::HttpResponse, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }

    async fn websocket_connect(
        &self,
        _req: crate::WebsocketRequest,
    ) -> core::result::Result<crate::WebsocketResponse, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }
}
