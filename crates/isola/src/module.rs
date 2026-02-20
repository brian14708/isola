use crate::TRACE_TARGET_SCRIPT;
use crate::{
    BoxError, Host, OutputSink, Result,
    error::Error,
    internal::sandbox::{
        InstanceState, SandboxPre,
        exports::GuestIndices,
        exports::{Argument as RawArgument, Value},
    },
};
use bytes::Bytes;
use component_init_transform::Invoker;
use futures::{FutureExt, Stream};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fmt::Write as _,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use tokio::time::timeout;
use tracing::{Instrument, info_span, warn};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Instance as WasmInstance},
};

// For `InstanceState::table()` when pushing iterator resources.
use crate::internal::sandbox::HostView as _;

const EPOCH_TICK: Duration = Duration::from_millis(10);
const ASYNC_YIELD_DEADLINE_TICKS: u64 = 1;

#[derive(Clone, Debug)]
pub struct DirectoryMapping {
    pub host: PathBuf,
    pub guest: String,
    pub writable: bool,
}

impl DirectoryMapping {
    #[must_use]
    pub fn new(host: impl Into<PathBuf>, guest: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            guest: guest.into(),
            writable: false,
        }
    }

    #[must_use]
    pub const fn writable(mut self, writable: bool) -> Self {
        self.writable = writable;
        self
    }
}

#[derive(Clone, Debug)]
pub struct ModuleConfig {
    /// Directory to store `.cwasm` artifacts. `None` disables caching.
    pub cache: Option<PathBuf>,
    pub max_memory: usize,
    /// Additional preopened directories for guest access.
    pub directory_mappings: Vec<DirectoryMapping>,
    pub prelude: Option<String>,
}

impl ModuleConfig {
    pub const DEFAULT_MAX_MEMORY: usize = 64 * 1024 * 1024;

    /// Minimal defaults without language-specific prelude setup.
    #[must_use]
    pub const fn minimal() -> Self {
        Self {
            cache: None,
            max_memory: Self::DEFAULT_MAX_MEMORY,
            directory_mappings: Vec::new(),
            prelude: None,
        }
    }

    /// Defaults for Python guest execution.
    #[must_use]
    pub fn python() -> Self {
        Self {
            prelude: Some("import sandbox.asyncio".to_string()),
            ..Self::minimal()
        }
    }
}

impl Default for ModuleConfig {
    fn default() -> Self {
        Self::minimal()
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

impl Arg {
    #[must_use]
    pub fn cbor(value: impl Into<Bytes>) -> Self {
        Self {
            name: None,
            value: ArgValue::Cbor(value.into()),
        }
    }

    /// Serialize a value into CBOR and use it as an unnamed argument.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    #[cfg(feature = "serde")]
    pub fn value<T: serde::Serialize>(value: &T) -> core::result::Result<Self, crate::cbor::Error> {
        let value = crate::cbor::to_cbor(value)?;
        Ok(Self::cbor(value))
    }

    #[must_use]
    pub fn cbor_stream(stream: impl Stream<Item = Bytes> + Send + 'static) -> Self {
        Self {
            name: None,
            value: ArgValue::CborStream(Box::pin(stream)),
        }
    }

    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

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

#[derive(Default)]
pub struct ModuleBuilder {
    config: ModuleConfig,
    engine_config: Option<Config>,
}

impl ModuleBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn config(mut self, cfg: ModuleConfig) -> Self {
        self.config = cfg;
        self
    }

    #[must_use]
    pub fn engine_config(mut self, cfg: Config) -> Self {
        self.engine_config = Some(cfg);
        self
    }

    /// # Errors
    /// Returns an error if the module cannot be built or compiled.
    pub async fn build<H: Host>(self, wasm: impl AsRef<Path>) -> Result<Module<H>> {
        let wasm_path = std::fs::canonicalize(wasm.as_ref()).map_err(Error::Io)?;
        let cfg = self.config;

        let custom_engine_config = self.engine_config.is_some();
        let mut engine_cfg = self.engine_config.unwrap_or_default();
        if custom_engine_config {
            warn!(
                "custom wasmtime::Config provided; isola will force required fields (component model, async support, epoch interruption)"
            );
        }
        configure_engine(&mut engine_cfg);
        let engine = Engine::new(&engine_cfg).map_err(Error::Wasm)?;

        let span = info_span!(target: TRACE_TARGET_SCRIPT, "module.build");
        let component = async {
            load_or_compile_component(&engine, &wasm_path, &cfg.directory_mappings, &cfg).await
        }
        .instrument(span)
        .await?;

        let linker = InstanceState::<H>::new_linker(&engine).map_err(Error::Wasm)?;
        let pre = linker.instantiate_pre(&component).map_err(Error::Wasm)?;
        Engine::tls_eager_initialize();
        let ticker = global_epoch_ticker()
            .map_err(Error::Io)?
            .register(engine.clone());

        Ok(Module {
            max_memory: cfg.max_memory,
            directory_mappings: cfg.directory_mappings,
            pre: SandboxPre::new(pre).map_err(Error::Wasm)?,
            ticker,
            engine,
        })
    }
}

/// Shared global epoch ticker state.
struct EpochTickerShared {
    engines: Mutex<HashMap<u64, Engine>>,
    next_id: AtomicU64,
}

struct GlobalEpochTicker {
    shared: Arc<EpochTickerShared>,
}

/// Registration that keeps epoch ticks active for a specific engine.
struct EpochTickerRegistration {
    id: u64,
    shared: Arc<EpochTickerShared>,
}

impl GlobalEpochTicker {
    fn new() -> std::io::Result<Self> {
        let shared = Arc::new(EpochTickerShared {
            engines: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        });

        let shared_bg = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("isola-epoch-ticker".to_string())
            .spawn(move || {
                // Keep epoch progression independent of Tokio scheduling.
                // This avoids timeout starvation in current-thread runtimes.
                loop {
                    std::thread::park_timeout(EPOCH_TICK);
                    let engines: Vec<Engine> = match shared_bg.engines.lock() {
                        Ok(engines) => engines.values().cloned().collect(),
                        Err(poisoned) => poisoned.into_inner().values().cloned().collect(),
                    };
                    for engine in engines {
                        engine.increment_epoch();
                    }
                }
            })?;

        Ok(Self { shared })
    }

    fn register(&self, engine: Engine) -> Arc<EpochTickerRegistration> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let mut engines = self
            .shared
            .engines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engines.insert(id, engine);
        drop(engines);

        Arc::new(EpochTickerRegistration {
            id,
            shared: Arc::clone(&self.shared),
        })
    }
}

impl Drop for EpochTickerRegistration {
    fn drop(&mut self) {
        let mut engines = self
            .shared
            .engines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engines.remove(&self.id);
    }
}

fn global_epoch_ticker() -> std::io::Result<&'static GlobalEpochTicker> {
    static GLOBAL_EPOCH_TICKER: OnceLock<
        core::result::Result<GlobalEpochTicker, (std::io::ErrorKind, String)>,
    > = OnceLock::new();

    let ticker = GLOBAL_EPOCH_TICKER
        .get_or_init(|| GlobalEpochTicker::new().map_err(|e| (e.kind(), e.to_string())));
    match ticker {
        Ok(ticker) => Ok(ticker),
        Err((kind, message)) => Err(std::io::Error::new(*kind, message.clone())),
    }
}

pub struct Module<H: Host> {
    max_memory: usize,
    directory_mappings: Vec<DirectoryMapping>,
    engine: Engine,
    pre: SandboxPre<InstanceState<H>>,
    ticker: Arc<EpochTickerRegistration>,
}

pub struct Sandbox<H: Host> {
    store: Store<InstanceState<H>>,
    bindings: crate::internal::sandbox::Sandbox,
    call_timeout: Option<Duration>,
    /// Keeps the epoch ticker alive for the lifetime of this sandbox.
    _ticker: Arc<EpochTickerRegistration>,
}

#[derive(Clone, Copy)]
pub struct SandboxOptions<'a> {
    pub log_level: tracing::level_filters::LevelFilter,
    pub directory_mappings: &'a [DirectoryMapping],
    pub call_timeout: Option<Duration>,
}

impl Default for SandboxOptions<'_> {
    fn default() -> Self {
        Self {
            log_level: tracing::level_filters::LevelFilter::OFF,
            directory_mappings: &[],
            call_timeout: None,
        }
    }
}

impl<'a> SandboxOptions<'a> {
    #[must_use]
    pub const fn log_level(mut self, log_level: tracing::level_filters::LevelFilter) -> Self {
        self.log_level = log_level;
        self
    }

    #[must_use]
    pub const fn directory_mappings(mut self, directory_mappings: &'a [DirectoryMapping]) -> Self {
        self.directory_mappings = directory_mappings;
        self
    }

    #[must_use]
    pub const fn call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = Some(timeout);
        self
    }
}

impl<H: Host> Module<H> {
    /// # Errors
    /// Returns an error if instantiation fails.
    pub async fn instantiate(&self, host: H, options: SandboxOptions<'_>) -> Result<Sandbox<H>> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.instantiate");
        let ticker = Arc::clone(&self.ticker);
        async move {
            let level = level_filter_to_wasi(options.log_level);
            let directory_mappings =
                merge_directory_mappings(&self.directory_mappings, options.directory_mappings);

            let mut store =
                InstanceState::new(&self.engine, &directory_mappings, self.max_memory, host)
                    .map_err(Error::Wasm)?;
            store.epoch_deadline_async_yield_and_update(ASYNC_YIELD_DEADLINE_TICKS);

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
                call_timeout: options.call_timeout,
                _ticker: ticker,
            })
        }
        .instrument(span)
        .await
    }
}

fn merge_directory_mappings(
    base: &[DirectoryMapping],
    extra: &[DirectoryMapping],
) -> Vec<DirectoryMapping> {
    let mut merged = base.to_vec();
    for mapping in extra {
        if let Some(existing) = merged.iter_mut().find(|m| m.guest == mapping.guest) {
            *existing = mapping.clone();
        } else {
            merged.push(mapping.clone());
        }
    }
    merged
}

/// RAII guard that clears the output sink when dropped, even if the call panics
/// or returns early.
struct CallCleanup<'a, H: Host> {
    store: &'a mut Store<InstanceState<H>>,
}

impl<H: Host> Drop for CallCleanup<'_, H> {
    fn drop(&mut self) {
        self.store.data_mut().set_sink(None);
    }
}

impl<H: Host> std::ops::Deref for CallCleanup<'_, H> {
    type Target = Store<InstanceState<H>>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl<H: Host> std::ops::DerefMut for CallCleanup<'_, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl<H: Host> wasmtime::AsContext for CallCleanup<'_, H> {
    type Data = InstanceState<H>;

    fn as_context(&self) -> wasmtime::StoreContext<'_, Self::Data> {
        wasmtime::AsContext::as_context(&*self.store)
    }
}

impl<H: Host> wasmtime::AsContextMut for CallCleanup<'_, H> {
    fn as_context_mut(&mut self) -> wasmtime::StoreContextMut<'_, Self::Data> {
        wasmtime::AsContextMut::as_context_mut(&mut *self.store)
    }
}

#[derive(Debug, Default)]
pub struct CallOutput {
    pub partials: Vec<Bytes>,
    pub result: Bytes,
}

struct CollectOutputSink {
    output: Arc<Mutex<CallOutput>>,
}

impl CollectOutputSink {
    const fn new(output: Arc<Mutex<CallOutput>>) -> Self {
        Self { output }
    }

    fn lock_output(&self) -> core::result::Result<std::sync::MutexGuard<'_, CallOutput>, BoxError> {
        self.output
            .lock()
            .map_err(|_| std::io::Error::other("collect output lock poisoned").into())
    }
}

#[async_trait::async_trait]
impl OutputSink for CollectOutputSink {
    async fn on_partial(&mut self, cbor: Bytes) -> core::result::Result<(), BoxError> {
        self.lock_output()?.partials.push(cbor);
        Ok(())
    }

    async fn on_end(&mut self, cbor: Bytes) -> core::result::Result<(), BoxError> {
        self.lock_output()?.result = cbor;
        Ok(())
    }
}

impl<H: Host> Sandbox<H> {
    /// # Errors
    /// Returns an error if the script evaluation fails.
    pub async fn eval_script(&mut self, code: impl AsRef<str>) -> Result<()> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.eval_script");
        let code = code.as_ref().to_string();
        let mut store = CallCleanup {
            store: &mut self.store,
        };
        self.bindings
            .isola_script_guest()
            .call_eval_script(&mut store, &code)
            .instrument(span)
            .await
            .map_err(Error::Wasm)??;
        Ok(())
    }

    /// Evaluate a file using its exact guest-visible path.
    ///
    /// # Errors
    /// Returns an error if the file evaluation fails.
    pub async fn eval_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.eval_file");
        let path = path.as_ref().to_string_lossy().to_string();
        let mut store = CallCleanup {
            store: &mut self.store,
        };
        self.bindings
            .isola_script_guest()
            .call_eval_file(&mut store, &path)
            .instrument(span)
            .await
            .map_err(Error::Wasm)??;
        Ok(())
    }

    /// # Errors
    /// Returns an error if the function execution fails.
    pub async fn call(&mut self, function: &str, args: Args, sink: impl OutputSink) -> Result<()> {
        self.call_impl(function, args, sink, self.call_timeout)
            .await
    }

    /// # Errors
    /// Returns an error if the function execution fails or times out.
    pub async fn call_with_timeout(
        &mut self,
        function: &str,
        args: Args,
        sink: impl OutputSink,
        timeout_duration: Duration,
    ) -> Result<()> {
        self.call_impl(function, args, sink, Some(timeout_duration))
            .await
    }

    /// # Errors
    /// Returns an error if the function execution fails.
    pub async fn call_collect(&mut self, function: &str, args: Args) -> Result<CallOutput> {
        self.call_collect_impl(function, args, self.call_timeout)
            .await
    }

    /// # Errors
    /// Returns an error if the function execution fails or times out.
    pub async fn call_collect_with_timeout(
        &mut self,
        function: &str,
        args: Args,
        timeout_duration: Duration,
    ) -> Result<CallOutput> {
        self.call_collect_impl(function, args, Some(timeout_duration))
            .await
    }

    async fn call_collect_impl(
        &mut self,
        function: &str,
        args: Args,
        timeout_duration: Option<Duration>,
    ) -> Result<CallOutput> {
        let output = Arc::new(Mutex::new(CallOutput::default()));
        let sink = CollectOutputSink::new(Arc::clone(&output));
        self.call_impl(function, args, sink, timeout_duration)
            .await?;

        let output = Arc::try_unwrap(output).map_err(|_| {
            Error::Host(std::io::Error::other("collect output still in use").into())
        })?;
        output
            .into_inner()
            .map_err(|_| Error::Host(std::io::Error::other("collect output lock poisoned").into()))
    }

    async fn call_impl(
        &mut self,
        function: &str,
        mut args: Args,
        sink: impl OutputSink,
        timeout_duration: Option<Duration>,
    ) -> Result<()> {
        let span = info_span!(target: TRACE_TARGET_SCRIPT, "sandbox.call");
        let call = async move {
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

            store.data_mut().set_sink(Some(Box::new(sink)));
            let result = self
                .bindings
                .isola_script_guest()
                .call_call_func(&mut store, &func, &internal_args)
                .await;
            store.data_mut().set_sink(None);

            result.map_err(Error::Wasm)??;
            Ok(())
        };

        let call = call.instrument(span);
        if let Some(timeout_duration) = timeout_duration {
            timeout(timeout_duration, call).await.map_err(|_| {
                Error::Wasm(anyhow::anyhow!(
                    "sandbox call timed out after {}ms",
                    timeout_duration.as_millis()
                ))
            })?
        } else {
            call.await
        }
    }

    #[must_use]
    pub fn memory_usage(&self) -> usize {
        self.store.data().limiter.current()
    }
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

fn configure_engine(cfg: &mut Config) {
    cfg.wasm_component_model(true);
    cfg.async_support(true);
    cfg.epoch_interruption(true);
    cfg.table_lazy_init(false);
    cfg.generate_address_map(false);
    cfg.wasm_backtrace(false);
    cfg.native_unwind_info(false);
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
}

async fn load_or_compile_component(
    engine: &Engine,
    wasm_path: &Path,
    directory_mappings: &[DirectoryMapping],
    cfg: &ModuleConfig,
) -> Result<Component> {
    let wasm_bytes = tokio::fs::read(wasm_path).await.map_err(Error::Io)?;

    let Some(cache_dir) = &cfg.cache else {
        let bytes =
            compile_serialized_component(engine, cfg, directory_mappings, &wasm_bytes).await?;
        // SAFETY: bytes are produced by wasmtime for the same version/config; if incompatible,
        // deserialization will fail and surface as an error.
        let component = unsafe { Component::deserialize(engine, &bytes) }.map_err(Error::Wasm)?;
        return Ok(component);
    };

    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(Error::Io)?;
    let key = cache_key(engine, cfg, &wasm_bytes);
    let cache_path = cache_dir.join(format!("{key}.cwasm"));

    if let Ok(component) = unsafe { Component::deserialize_file(engine, &cache_path) } {
        return Ok(component);
    }

    let bytes = compile_serialized_component(engine, cfg, directory_mappings, &wasm_bytes).await?;
    write_cache_file_atomic(&cache_path, &bytes).await?;

    let component =
        unsafe { Component::deserialize_file(engine, &cache_path) }.map_err(Error::Wasm)?;
    Ok(component)
}

async fn write_cache_file_atomic(cache_path: &Path, bytes: &[u8]) -> Result<()> {
    static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = CACHE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path =
        cache_path.with_extension(format!("cwasm.tmp-{}-{sequence}", std::process::id()));

    tokio::fs::write(&tmp_path, bytes)
        .await
        .map_err(Error::Io)?;
    match tokio::fs::rename(&tmp_path, cache_path).await {
        Ok(()) => Ok(()),
        // Windows doesn't atomically replace by default; treat a concurrent winner as success.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            Ok(())
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            Err(Error::Io(e))
        }
    }
}

fn engine_fingerprint(engine: &Engine) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
}

fn cache_key(engine: &Engine, cfg: &ModuleConfig, wasm_bytes: &[u8]) -> String {
    let mut wasm_h = Sha256::new();
    wasm_h.update(wasm_bytes);
    let wasm_digest = wasm_h.finalize();

    let mut h = Sha256::new();
    h.update(b"isola-cache-v0\0");
    h.update(wasm_digest);
    h.update(engine_fingerprint(engine).to_le_bytes());

    h.update((cfg.directory_mappings.len() as u64).to_le_bytes());
    for mapping in &cfg.directory_mappings {
        h.update(mapping.guest.as_bytes());
        h.update([0]);
        let host = mapping.host.to_string_lossy();
        h.update(host.as_bytes());
        h.update([0]);
        h.update([u8::from(mapping.writable)]);
    }

    if let Some(prelude) = &cfg.prelude {
        h.update([1]);
        h.update(prelude.as_bytes());
    } else {
        h.update([0]);
    }

    h.update((cfg.max_memory as u64).to_le_bytes());
    // Optimization level is fixed in `configure_engine`.
    h.update([1]);

    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

async fn compile_serialized_component(
    engine: &Engine,
    cfg: &ModuleConfig,
    directory_mappings: &[DirectoryMapping],
    wasm_bytes: &[u8],
) -> Result<Vec<u8>> {
    let engine = engine.clone();
    let cfg = cfg.clone();
    let directory_mappings = directory_mappings.to_vec();
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
                        &directory_mappings,
                        cfg.max_memory,
                        CompileHost,
                    )
                    .map_err(Error::Wasm)?;
                    store.epoch_deadline_async_yield_and_update(ASYNC_YIELD_DEADLINE_TICKS);

                    let pre = linker.instantiate_pre(&component).map_err(Error::Wasm)?;
                    let binding = pre
                        .instantiate_async(&mut store)
                        .await
                        .map_err(Error::Wasm)?;
                    let guest = GuestIndices::new(&pre)
                        .map_err(Error::Wasm)?
                        .load(&mut store, &binding)
                        .map_err(Error::Wasm)?;

                    guest
                        .call_initialize(&mut store, true, cfg.prelude.as_deref())
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

struct MyInvoker<S: Host> {
    store: Store<InstanceState<S>>,
    instance: WasmInstance,
}

#[async_trait::async_trait]
impl<S: Host> Invoker for MyInvoker<S> {
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
}
