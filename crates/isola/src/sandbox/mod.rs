//! Runtime module lifecycle APIs.
//!
//! Typical flow:
//! 1. Build a [`SandboxTemplate`](crate::sandbox::SandboxTemplate) with
//!    [`SandboxTemplateBuilder`](crate::sandbox::SandboxTemplateBuilder).
//! 2. Instantiate a [`Sandbox`](crate::sandbox::Sandbox) from that template and
//!    host implementation.
//! 3. Evaluate scripts or call guest functions with
//!    [`Arg`](crate::sandbox::Arg) inputs.
//!
//! [`SandboxOptions`](crate::sandbox::SandboxOptions) controls
//! per-instantiation options (for example mount/env overrides and per-sandbox
//! memory cap), while template defaults are configured via
//! [`SandboxTemplateBuilder`](crate::sandbox::SandboxTemplateBuilder).

#[cfg(feature = "serde")]
mod args_macro;

use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use futures::{Stream, stream};
use parking_lot::Mutex;
use smallvec::SmallVec;
use wasmtime::{Engine, Store};
pub use wasmtime_wasi::{DirPerms, FilePerms};

#[cfg(feature = "serde")]
pub use crate::args;
use crate::{
    host::{Host, OutputSink},
    internal::{
        module::{
            ModuleConfig as InternalModuleConfig,
            call::CallCleanup,
            compile::load_or_compile_component,
            configure::configure_engine,
            epoch::{EpochTickerRegistration, global_epoch_ticker},
        },
        sandbox::{
            HostView as _, InstanceState, Sandbox as WasmSandbox, SandboxPre, ValueIterator,
            exports::{self, Argument as RawArgument, Value as WasmValue},
        },
    },
    value::Value,
};

/// Result type used by `isola::sandbox` APIs.
pub type Result<T, E = Error> = core::result::Result<T, E>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Guest/user-code failure.
    #[error("{message}")]
    UserCode { message: String },

    /// Internal runtime failure (wasm engine, host callback, filesystem, etc).
    #[error("runtime error: {0}")]
    Runtime(#[source] anyhow::Error),
}

impl From<exports::Error> for Error {
    fn from(value: exports::Error) -> Self {
        let exports::Error { code, message } = value;
        match code {
            exports::ErrorCode::Aborted => Self::UserCode { message },
            exports::ErrorCode::Unknown | exports::ErrorCode::Internal => {
                Self::Runtime(anyhow::anyhow!("[{code:?}] {message}"))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DirectoryMapping {
    pub(crate) host: PathBuf,
    pub(crate) guest: String,
    pub(crate) dir_perms: DirPerms,
    pub(crate) file_perms: FilePerms,
}

impl DirectoryMapping {
    pub fn new(host: impl Into<PathBuf>, guest: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            guest: guest.into(),
            dir_perms: DirPerms::READ,
            file_perms: FilePerms::READ,
        }
    }

    pub const fn with_permissions(mut self, dir_perms: DirPerms, file_perms: FilePerms) -> Self {
        self.dir_perms = dir_perms;
        self.file_perms = file_perms;
        self
    }
}

/// Function argument passed to guest `call-func`.
pub enum Arg {
    Positional(Value),
    Named(String, Value),
    PositionalStream(Pin<Box<dyn Stream<Item = Value> + Send + 'static>>),
    NamedStream(String, Pin<Box<dyn Stream<Item = Value> + Send + 'static>>),
}

impl core::fmt::Debug for Arg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Positional(value) => f
                .debug_struct("Arg::Positional")
                .field("value", value)
                .finish(),
            Self::Named(name, value) => f
                .debug_struct("Arg::Named")
                .field("name", name)
                .field("value", value)
                .finish(),
            Self::PositionalStream(..) => f
                .debug_struct("Arg::PositionalStream")
                .field("stream", &"<stream>")
                .finish(),
            Self::NamedStream(name, ..) => f
                .debug_struct("Arg::NamedStream")
                .field("name", name)
                .field("stream", &"<stream>")
                .finish(),
        }
    }
}

/// Builder for compiling a reusable [`SandboxTemplate`].
///
/// `SandboxTemplateBuilder` configures template-level defaults shared by every
/// sandbox instantiated from the resulting [`SandboxTemplate`], including base
/// mount/env settings.
#[derive(Default)]
pub struct SandboxTemplateBuilder {
    pub(crate) cache: Option<PathBuf>,
    pub(crate) base_options: SandboxOptions,
    pub(crate) prelude: Option<String>,
}

/// Compiled sandbox template that can instantiate multiple sandboxes.
///
/// A `SandboxTemplate` is an immutable, reusable compiled artifact.
/// Instantiate it to create one or more independent [`Sandbox`] values with
/// isolated runtime state.
pub struct SandboxTemplate<H: Host> {
    pub(crate) base_options: SandboxOptions,
    pub(crate) engine: Engine,
    pub(crate) pre: SandboxPre<InstanceState<H>>,
    pub(crate) ticker: Arc<EpochTickerRegistration>,
}

/// Live guest instance with mutable execution state.
///
/// A `Sandbox` belongs to a single instantiation of a [`SandboxTemplate`]. It
/// carries the guest state for eval/call operations and is isolated from other
/// sandboxes created from the same template.
pub struct Sandbox<H: Host> {
    pub(crate) store: Store<InstanceState<H>>,
    pub(crate) bindings: WasmSandbox,
    /// Keeps the epoch ticker alive for the lifetime of this sandbox.
    pub(crate) _ticker: Arc<EpochTickerRegistration>,
}

/// Per-instantiation options when creating a
/// [`Sandbox`] from a [`SandboxTemplate`]
#[derive(Clone, Debug, Default)]
pub struct SandboxOptions {
    pub(crate) max_memory: Option<usize>,
    pub(crate) directory_mappings: Vec<DirectoryMapping>,
    pub(crate) env: Vec<(String, String)>,
}

impl SandboxOptions {
    /// Override the memory hard limit for this sandbox.
    pub const fn max_memory(&mut self, max_memory: usize) -> &mut Self {
        self.max_memory = Some(max_memory);
        self
    }

    /// Mount a host directory into this sandbox instance.
    ///
    /// If a guest path duplicates a module-level mount, this mount replaces it
    /// for that instance.
    pub fn mount(
        &mut self,
        host_path: impl AsRef<Path>,
        guest_path: impl AsRef<str>,
        dir_perms: DirPerms,
        file_perms: FilePerms,
    ) -> &mut Self {
        self.directory_mappings.push(
            DirectoryMapping::new(host_path.as_ref(), guest_path.as_ref())
                .with_permissions(dir_perms, file_perms),
        );
        self
    }

    /// Add an environment variable for this sandbox instance.
    ///
    /// If the same key is set multiple times, the last value wins.
    pub fn env(&mut self, k: impl AsRef<str>, v: impl AsRef<str>) -> &mut Self {
        self.env
            .push((k.as_ref().to_string(), v.as_ref().to_string()));
        self
    }

    /// Merge `overrides` into this options value and return the merged result.
    ///
    /// Merge behavior:
    /// - `max_memory`: override wins when set.
    /// - mounts: override entries replace on guest-path collision.
    /// - `env`: override values replace by matching key.
    #[must_use]
    pub fn merged_with(&self, overrides: &Self) -> Self {
        let mut merged = self.clone();

        if let Some(max_memory) = overrides.max_memory {
            merged.max_memory = Some(max_memory);
        }

        for mapping in &overrides.directory_mappings {
            if let Some(existing) = merged
                .directory_mappings
                .iter_mut()
                .find(|m| m.guest == mapping.guest)
            {
                *existing = mapping.clone();
            } else {
                merged.directory_mappings.push(mapping.clone());
            }
        }

        for (key, value) in &overrides.env {
            if let Some(existing) = merged.env.iter_mut().find(|(k, _)| k == key) {
                existing.1.clone_from(value);
            } else {
                merged.env.push((key.clone(), value.clone()));
            }
        }

        merged
    }
}

/// Collected output from
/// [`Sandbox::call`](crate::sandbox::Sandbox::call).
#[derive(Debug, Default)]
pub struct CallOutput {
    pub items: Vec<Value>,
    pub result: Option<Value>,
}

impl SandboxTemplateBuilder {
    /// Set the optional component cache directory.
    ///
    /// When set, compiled artifacts are cached on disk and reused across
    /// builds.
    #[must_use]
    pub fn cache(mut self, cache: Option<std::path::PathBuf>) -> Self {
        self.cache = cache;
        self
    }

    /// Set the per-sandbox memory hard limit.
    ///
    /// Defaults to unlimited (`usize::MAX`).
    #[must_use]
    pub const fn max_memory(mut self, max_memory: usize) -> Self {
        self.base_options.max_memory = Some(max_memory);
        self
    }

    /// Set base directory mappings shared by all sandboxes from this template.
    ///
    /// These mappings can be extended or overridden per instantiation via
    /// [`SandboxOptions::mount`](crate::sandbox::SandboxOptions::mount).
    ///
    /// This API matches the WASI-style preopen configuration shape.
    #[must_use]
    pub fn mount(
        mut self,
        host_path: impl AsRef<Path>,
        guest_path: impl AsRef<str>,
        dir_perms: DirPerms,
        file_perms: FilePerms,
    ) -> Self {
        self.base_options
            .mount(host_path, guest_path, dir_perms, file_perms);
        self
    }

    /// Add an environment variable that will be present in sandbox WASI env.
    ///
    /// If the same key is set multiple times, the last value wins.
    pub fn env(&mut self, k: impl AsRef<str>, v: impl AsRef<str>) -> &mut Self {
        self.base_options.env(k, v);
        self
    }

    /// Set optional guest prelude code executed during template initialization.
    #[must_use]
    pub fn prelude(mut self, prelude: Option<String>) -> Self {
        self.prelude = prelude;
        self
    }

    /// # Errors
    /// Returns an error if the template cannot be built or compiled.
    pub async fn build<H: Host>(self, wasm: impl AsRef<Path>) -> Result<SandboxTemplate<H>> {
        let wasm_path =
            std::fs::canonicalize(wasm.as_ref()).map_err(|e| Error::Runtime(e.into()))?;
        let base_options = self.base_options;
        let cfg = InternalModuleConfig {
            cache: self.cache.clone(),
            max_memory: base_options.max_memory.unwrap_or(usize::MAX),
            directory_mappings: base_options.directory_mappings.clone(),
            env: base_options.env.clone(),
            prelude: self.prelude.clone(),
        };

        let mut engine_cfg = wasmtime::Config::default();
        configure_engine(&mut engine_cfg);
        let engine = Engine::new(&engine_cfg).map_err(Error::Runtime)?;

        let component =
            load_or_compile_component(&engine, &wasm_path, &cfg.directory_mappings, &cfg).await?;

        let linker = InstanceState::<H>::new_linker(&engine).map_err(Error::Runtime)?;
        let pre = linker.instantiate_pre(&component).map_err(Error::Runtime)?;
        Engine::tls_eager_initialize();
        let ticker = global_epoch_ticker()
            .map_err(|e| Error::Runtime(e.into()))?
            .register(engine.clone());

        Ok(SandboxTemplate {
            base_options,
            pre: SandboxPre::new(pre).map_err(Error::Runtime)?,
            ticker,
            engine,
        })
    }
}

impl<H: Host> SandboxTemplate<H> {
    /// Create a builder for this host type.
    #[must_use]
    pub fn builder() -> SandboxTemplateBuilder {
        SandboxTemplateBuilder::default()
    }

    /// Create a new sandbox instance from this compiled template.
    ///
    /// Each sandbox has isolated mutable guest state. Per-sandbox
    /// [`SandboxOptions`] are merged with the template defaults configured on
    /// [`SandboxTemplateBuilder`].
    ///
    /// # Errors
    /// Returns an error if instantiation fails.
    pub async fn instantiate(&self, host: H, options: SandboxOptions) -> Result<Sandbox<H>> {
        let ticker = Arc::clone(&self.ticker);
        let merged = self.base_options.merged_with(&options);

        let mut store = InstanceState::new(
            &self.engine,
            &merged.directory_mappings,
            &merged.env,
            merged.max_memory.unwrap_or(usize::MAX),
            host,
        )
        .map_err(Error::Runtime)?;
        store.epoch_deadline_async_yield_and_update(1);

        let bindings = self
            .pre
            .instantiate_async(&mut store)
            .await
            .map_err(Error::Runtime)?;

        Ok(Sandbox {
            store,
            bindings,
            _ticker: ticker,
        })
    }
}

impl<H: Host> Sandbox<H> {
    /// # Errors
    /// Returns an error if the script evaluation fails.
    pub async fn eval_script(
        &mut self,
        code: impl AsRef<str>,
        sink: Arc<dyn OutputSink>,
    ) -> Result<()> {
        let code = code.as_ref().to_string();
        let mut store = CallCleanup::new(&mut self.store);
        store.set_sink(Arc::clone(&sink));
        let result = self
            .bindings
            .isola_script_runtime()
            .call_eval_script(&mut store, &code)
            .await;
        let flush_result = store.data_mut().flush_logs().await.map_err(Error::Runtime);
        result.map_err(Error::Runtime)??;
        flush_result?;
        Ok(())
    }

    /// Evaluate a file using its exact guest-visible path string.
    ///
    /// # Errors
    /// Returns an error if the file evaluation fails.
    pub async fn eval_file(&mut self, guest_path: &str, sink: Arc<dyn OutputSink>) -> Result<()> {
        let mut store = CallCleanup::new(&mut self.store);
        store.set_sink(Arc::clone(&sink));
        let result = self
            .bindings
            .isola_script_runtime()
            .call_eval_file(&mut store, guest_path)
            .await;
        let flush_result = store.data_mut().flush_logs().await.map_err(Error::Runtime);
        result.map_err(Error::Runtime)??;
        flush_result?;
        Ok(())
    }

    /// Call a guest function and deliver output incrementally to a sink.
    ///
    /// # Errors
    /// Returns an error if the function execution fails.
    pub async fn call_with_sink<I>(
        &mut self,
        function: &str,
        args: I,
        sink: Arc<dyn OutputSink>,
    ) -> Result<()>
    where
        I: IntoIterator<Item = Arg>,
    {
        self.call_impl(function, args, sink).await
    }

    /// Call a guest function and collect emitted items/final result.
    ///
    /// # Errors
    /// Returns an error if the function execution fails.
    pub async fn call<I>(&mut self, function: &str, args: I) -> Result<CallOutput>
    where
        I: IntoIterator<Item = Arg>,
    {
        let output = Arc::new(Mutex::new(CallOutput::default()));
        let sink: Arc<dyn OutputSink> = output.clone();
        self.call_impl(function, args, sink).await?;

        let mut output = output.lock();
        Ok(std::mem::take(&mut output))
    }

    async fn call_impl<I>(
        &mut self,
        function: &str,
        args: I,
        sink: Arc<dyn OutputSink>,
    ) -> Result<()>
    where
        I: IntoIterator<Item = Arg>,
    {
        let mut args: SmallVec<[Arg; 2]> = args.into_iter().collect();
        let mut store = CallCleanup::new(&mut self.store);
        let internal_args = args
            .iter_mut()
            .map(|arg| match arg {
                Arg::Positional(value) => Ok(RawArgument {
                    name: None,
                    value: WasmValue::Cbor(value.as_cbor()),
                }),
                Arg::Named(name, value) => Ok(RawArgument {
                    name: Some(name.as_str()),
                    value: WasmValue::Cbor(value.as_cbor()),
                }),
                Arg::PositionalStream(stream_arg) => {
                    let stream = std::mem::replace(stream_arg, Box::pin(stream::empty()));
                    let iter = store
                        .data_mut()
                        .table()
                        .push(ValueIterator::new(stream))
                        .map_err(|e| Error::Runtime(e.into()))?;
                    Ok(RawArgument {
                        name: None,
                        value: WasmValue::CborIterator(iter),
                    })
                }
                Arg::NamedStream(name, stream_arg) => {
                    let stream = std::mem::replace(stream_arg, Box::pin(stream::empty()));
                    let iter = store
                        .data_mut()
                        .table()
                        .push(ValueIterator::new(stream))
                        .map_err(|e| Error::Runtime(e.into()))?;
                    Ok(RawArgument {
                        name: Some(name.as_str()),
                        value: WasmValue::CborIterator(iter),
                    })
                }
            })
            .collect::<Result<SmallVec<[RawArgument; 2]>>>()?;

        store.set_sink(sink);
        let result = self
            .bindings
            .isola_script_runtime()
            .call_call_func(&mut store, function, &internal_args)
            .await;
        let flush_result = store.data_mut().flush_logs().await.map_err(Error::Runtime);
        result.map_err(Error::Runtime)??;
        flush_result?;
        Ok(())
    }

    #[must_use]
    pub fn memory_usage(&self) -> usize {
        self.store.data().limiter.current()
    }
}
