use std::{
    ffi::{CStr, c_char, c_int, c_void},
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use isola::{
    host::{BoxError, LogContext, LogLevel, OutputSink},
    sandbox::{Arg, DirPerms, FilePerms, Sandbox, SandboxOptions, SandboxTemplate},
    value::Value,
};
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};

use crate::{
    env::Env,
    error::{Error, ErrorCode, Result},
};

mod env;
mod error;

macro_rules! c_try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(e) => {
                let code = $crate::error::ErrorCode::from(&e);
                $crate::error::set_last_error(e);
                return code;
            }
        }
    };
}

unsafe fn require_ref<'a, T>(ptr: *const T, message: &'static str) -> Result<&'a T> {
    unsafe { ptr.as_ref() }.ok_or(Error::InvalidArgument(message))
}

unsafe fn require_mut<'a, T>(ptr: *mut T, message: &'static str) -> Result<&'a mut T> {
    unsafe { ptr.as_mut() }.ok_or(Error::InvalidArgument(message))
}

const unsafe fn require_cstr<'a>(ptr: *const c_char, message: &'static str) -> Result<&'a CStr> {
    if ptr.is_null() {
        return Err(Error::InvalidArgument(message));
    }
    Ok(unsafe { CStr::from_ptr(ptr) })
}

fn fail(error: Error) -> ErrorCode {
    let code = ErrorCode::from(&error);
    crate::error::set_last_error(error);
    code
}

// ---------------------------------------------------------------------------
// Unified sandbox handler vtable
// ---------------------------------------------------------------------------

/// Unified vtable for sandbox event handling and optional HTTP support.
///
/// - `on_event` is required.
/// - `http_request` is optional (NULL to disable HTTP).
///
/// Callbacks may run on runtime worker threads. The callback functions and
/// `user_data` must remain valid until the sandbox is destroyed and must be
/// safe to access concurrently.
#[repr(C)]
pub struct SandboxHandlerVtable {
    /// Called for output events (results, logs, stdout/stderr).
    ///
    /// `data` is valid only for the duration of the callback. It may be `NULL`
    /// only when `len` is zero.
    pub on_event: Option<extern "C" fn(CallbackEvent, *const u8, usize, *mut c_void)>,

    /// Called to initiate an HTTP request.
    ///
    /// The callback should return immediately. The `response_body` handle
    /// is Rust-owned; the C side completes the response asynchronously:
    ///
    /// 1. `isola_http_response_body_start(response_body, status)` — once
    ///    headers arrive.
    /// 2. `isola_http_response_body_push(response_body, data, len)` — for each
    ///    body chunk.
    /// 3. `isola_http_response_body_close(response_body)` — to signal EOF and
    ///    free the handle.
    pub http_request: Option<
        extern "C" fn(
            request: *const crate::env::HttpRequestInfo,
            response_body: *mut crate::env::HttpResponseBody,
            user_data: *mut c_void,
        ),
    >,

    /// Called to handle a hostcall from guest code.
    ///
    /// The callback should return immediately. The `response` handle
    /// is Rust-owned; the C side delivers the result asynchronously:
    ///
    /// - `isola_hostcall_response_resolve(response, data, len)` — on success
    /// - `isola_hostcall_response_reject(response, error_message)` — on failure
    ///
    /// Exactly one of resolve/reject/cancel must be called. The call frees the
    /// handle.
    pub hostcall: Option<
        extern "C" fn(
            call_type: *const c_char,
            payload: *const u8,
            payload_len: usize,
            response: *mut crate::env::HostcallResponse,
            user_data: *mut c_void,
        ),
    >,
}

/// Resolved handler: vtable + `user_data`, stored internally.
pub struct SandboxHandler {
    pub vtable: SandboxHandlerVtable,
    pub on_event: extern "C" fn(CallbackEvent, *const u8, usize, *mut c_void),
    pub user_data: *mut c_void,
}

// SAFETY: The C consumer guarantees thread-safe access to the callback
// pointers and `user_data`.
unsafe impl Send for SandboxHandler {}
unsafe impl Sync for SandboxHandler {}

#[async_trait]
impl OutputSink for SandboxHandler {
    async fn on_item(&self, item: Value) -> std::result::Result<(), BoxError> {
        let data = item.to_json_str().map_err(|e| -> BoxError {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
        (self.on_event)(
            CallbackEvent::ResultJson,
            data.as_ptr(),
            data.len(),
            self.user_data,
        );
        Ok(())
    }

    async fn on_complete(&self, item: Option<Value>) -> std::result::Result<(), BoxError> {
        if let Some(item) = item {
            let data = item.to_json_str().map_err(|e| -> BoxError {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
            (self.on_event)(
                CallbackEvent::EndJson,
                data.as_ptr(),
                data.len(),
                self.user_data,
            );
        } else {
            (self.on_event)(CallbackEvent::EndJson, std::ptr::null(), 0, self.user_data);
        }
        Ok(())
    }

    async fn on_log(
        &self,
        level: LogLevel,
        _log_context: LogContext<'_>,
        message: &str,
    ) -> std::result::Result<(), BoxError> {
        let event = match level {
            LogLevel::Stdout => CallbackEvent::Stdout,
            LogLevel::Stderr => CallbackEvent::Stderr,
            _ => CallbackEvent::Log,
        };
        (self.on_event)(event, message.as_ptr(), message.len(), self.user_data);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// JSON schema for the `"env"` config key: `{"name": "VAR", "value": "val"}`
#[derive(Deserialize)]
struct EnvConfig {
    name: String,
    value: String,
}

/// JSON schema for the `"mount"` config key:
/// `{"host": "/host/path", "guest": "/guest/path", "writable": false}`
#[derive(Deserialize)]
struct MountConfig {
    host: String,
    guest: String,
    #[serde(default)]
    writable: bool,
}

impl MountConfig {
    fn dir_perms(&self) -> DirPerms {
        if self.writable {
            DirPerms::READ | DirPerms::MUTATE
        } else {
            DirPerms::READ
        }
    }

    fn file_perms(&self) -> FilePerms {
        if self.writable {
            FilePerms::READ | FilePerms::WRITE
        } else {
            FilePerms::READ
        }
    }
}

const DEFAULT_MAX_MEMORY: usize = 64 * 1024 * 1024;
const DEFAULT_PRELUDE: &str = "import sandbox.asyncio";

#[derive(Default)]
struct ContextConfig {
    max_memory: Option<usize>,
    prelude: Option<String>,
    cache: Option<String>,
    env: Vec<(String, String)>,
    mounts: Vec<MountConfig>,
}

struct ContextCore {
    rt: Runtime,
    module: Option<SandboxTemplate>,
    config: ContextConfig,
}

impl ContextCore {
    fn create_handle(nr_thread: i32) -> Result<Box<ContextHandle>> {
        let rt = match nr_thread {
            0 => Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?,

            n if n > 0 => Builder::new_multi_thread()
                .worker_threads(
                    n.try_into()
                        .map_err(|_| Error::InvalidArgument("Invalid thread count"))?,
                )
                .thread_name("isola-runner")
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?,

            _ => Builder::new_multi_thread()
                .thread_name("isola-runner")
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?,
        };
        Ok(Box::new(ContextHandle(Arc::new(Self {
            rt,
            module: None,
            config: ContextConfig::default(),
        }))))
    }

    fn set_config(&mut self, key: &CStr, value: &CStr) -> Result<()> {
        if self.module.is_some() {
            return Err(Error::InvalidArgument(
                "Cannot set config after initialization",
            ));
        }
        let key = key
            .to_str()
            .map_err(|_| Error::InvalidArgument("Invalid UTF-8 in config key"))?;
        let value = value
            .to_str()
            .map_err(|_| Error::InvalidArgument("Invalid UTF-8 in config value"))?;

        match key {
            "max_memory" => {
                let bytes: usize = value
                    .parse()
                    .map_err(|_| Error::InvalidArgument("Invalid max_memory value"))?;
                self.config.max_memory = Some(bytes);
            }
            "prelude" => {
                self.config.prelude = Some(value.to_string());
            }
            "cache" => {
                self.config.cache = Some(value.to_string());
            }
            "env" => {
                let env: EnvConfig = serde_json::from_str(value)
                    .map_err(|_| Error::InvalidArgument("Invalid JSON for env"))?;
                self.config.env.push((env.name, env.value));
            }
            "mount" => {
                let mount: MountConfig = serde_json::from_str(value)
                    .map_err(|_| Error::InvalidArgument("Invalid JSON for mount"))?;
                self.config.mounts.push(mount);
            }
            _ => return Err(Error::InvalidArgument("Unknown config key")),
        }
        Ok(())
    }

    fn load(&mut self, path: &str) -> Result<()> {
        if self.module.is_some() {
            return Err(Error::InvalidArgument("Runtime already loaded"));
        }
        let path = PathBuf::from(path);

        let parent = path
            .parent()
            .ok_or_else(|| Error::Internal("Wasm path has no parent directory".to_string()))?;
        let mut lib_dir = std::env::var("WASI_PYTHON_RUNTIME").map_or_else(
            |_| {
                let mut lib_dir = parent.to_owned();
                lib_dir.push("wasm32-wasip1");
                lib_dir.push("wasi-deps");
                lib_dir.push("usr");
                lib_dir.push("local");
                lib_dir
            },
            PathBuf::from,
        );
        lib_dir.push("lib");

        let max_memory = self.config.max_memory.unwrap_or(DEFAULT_MAX_MEMORY);
        let prelude = match self.config.prelude.take() {
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s),
            None => Some(DEFAULT_PRELUDE.to_string()),
        };
        let cache = self
            .config
            .cache
            .take()
            .map_or_else(|| parent.join("cache"), PathBuf::from);

        self.rt.block_on(async {
            let mut builder = SandboxTemplate::builder()
                .prelude(prelude)
                .cache(Some(cache))
                .max_memory(max_memory)
                .mount(&lib_dir, "/lib", DirPerms::READ, FilePerms::READ);

            for mount in &self.config.mounts {
                builder = builder.mount(
                    &mount.host,
                    &mount.guest,
                    mount.dir_perms(),
                    mount.file_perms(),
                );
            }

            for (k, v) in &self.config.env {
                builder = builder.env(k, v);
            }

            let module = builder
                .build(&path)
                .await
                .map_err(|e| Error::Internal(format!("Failed to load runtime: {e}")))?;
            self.module = Some(module);
            Ok(())
        })
    }

    fn new_sandbox(self: &Arc<Self>) -> Result<SandboxHandle> {
        if self.module.is_none() {
            return Err(Error::InvalidArgument("Runtime not loaded"));
        }
        Ok(SandboxHandle {
            ctx: self.clone(),
            handler_slot: Arc::new(OnceLock::new()),
            inner: SandboxInner::Pending {
                options: SandboxOptions::default(),
            },
        })
    }
}

pub struct ContextHandle(Arc<ContextCore>);

/// Creates a new isola context with the specified number of threads.
///
/// # Safety
///
/// `out_context` must point to writable handle storage. It is set to `NULL`
/// before context creation, so it remains `NULL` if creation fails.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_create(
    nr_thread: c_int,
    out_context: *mut *mut ContextHandle,
) -> ErrorCode {
    let out_context = c_try!(unsafe { require_mut(out_context, "out_context must not be NULL") });
    *out_context = std::ptr::null_mut();
    let ctx = c_try!(ContextCore::create_handle(nr_thread));
    *out_context = Box::into_raw(ctx);
    ErrorCode::Ok
}

/// Initializes the isola context with the specified path.
///
/// # Safety
///
/// `ctx` must be a live context handle and `path` must be a valid,
/// null-terminated C string. `NULL` arguments are rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_initialize(
    ctx: *mut ContextHandle,
    path: *const c_char,
) -> ErrorCode {
    let ctx = c_try!(unsafe { require_mut(ctx, "ctx must not be NULL") });
    let Some(ctx) = Arc::get_mut(&mut ctx.0) else {
        return fail(Error::InvalidArgument(
            "context is shared by one or more sandboxes",
        ));
    };
    let path = c_try!(unsafe { require_cstr(path, "path must not be NULL") });
    let path = c_try!(
        path.to_str()
            .map_or_else(|_| Err(Error::InvalidArgument("Invalid path string")), Ok)
    );
    c_try!(ctx.load(path));
    ErrorCode::Ok
}

/// Sets a configuration value for the isola context.
///
/// Must be called **before** `isola_context_initialize`. Returns
/// `ISOLA_ERROR_CODE_INVALID_ARGUMENT` if the context is already initialized
/// or if `key` is unrecognized.
///
/// # Supported keys
///
/// | Key        | Value                                                | Default                         |
/// |------------|------------------------------------------------------|---------------------------------|
/// | `max_memory` | Decimal byte count (e.g. `"67108864"` for 64 MiB). | `67108864` (64 MiB)            |
/// | `prelude`  | Guest prelude source code. Empty string disables it. | `"import sandbox.asyncio"`      |
/// | `cache`    | Path to the compiled-component cache directory.      | `<wasm_dir>/cache`              |
/// | `env`      | JSON: `{"name":"VAR","value":"val"}`                 | *(none)*                        |
/// | `mount`    | JSON: `{"host":"/h","guest":"/g","writable":false}`  | *(none; `/lib` always mounted)* |
///
/// The `env` and `mount` keys may be called multiple times to add multiple
/// entries. For `mount`, `writable` defaults to `false` when omitted.
///
/// # Safety
///
/// The caller must ensure that both `key` and `value` are valid,
/// null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_config_set(
    ctx: *mut ContextHandle,
    key: *const c_char,
    value: *const c_char,
) -> ErrorCode {
    let ctx = c_try!(unsafe { require_mut(ctx, "ctx must not be NULL") });
    let Some(ctx) = Arc::get_mut(&mut ctx.0) else {
        return fail(Error::InvalidArgument(
            "context is shared by one or more sandboxes",
        ));
    };
    let key = c_try!(unsafe { require_cstr(key, "key must not be NULL") });
    let value = c_try!(unsafe { require_cstr(value, "value must not be NULL") });
    c_try!(ctx.set_config(key, value));
    ErrorCode::Ok
}

/// Destroy a context handle. Passing `NULL` is allowed and has no effect.
///
/// # Safety
/// A non-`NULL` handle must have been returned by `isola_context_create` and
/// must not have been destroyed previously.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_destroy(ctx: *mut ContextHandle) {
    if !ctx.is_null() {
        drop(unsafe { Box::from_raw(ctx) });
    }
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

enum SandboxInner {
    Uninitialized,
    Pending {
        options: SandboxOptions,
    },
    Running {
        sandbox: Sandbox<Env>,
        handler: Arc<SandboxHandler>,
    },
}

pub struct SandboxHandle {
    ctx: Arc<ContextCore>,
    handler_slot: Arc<OnceLock<Arc<SandboxHandler>>>,
    inner: SandboxInner,
}

impl SandboxHandle {
    fn set_config(&mut self, key: &CStr, value: &CStr) -> Result<()> {
        let SandboxInner::Pending { options } = &mut self.inner else {
            return Err(Error::InvalidArgument("Cannot set config after start"));
        };
        let key = key
            .to_str()
            .map_err(|_| Error::InvalidArgument("Invalid UTF-8 in config key"))?;
        let value = value
            .to_str()
            .map_err(|_| Error::InvalidArgument("Invalid UTF-8 in config value"))?;

        match key {
            "max_memory" => {
                let bytes: usize = value
                    .parse()
                    .map_err(|_| Error::InvalidArgument("Invalid max_memory value"))?;
                *options = std::mem::take(options).max_memory(bytes);
            }
            "env" => {
                let env: EnvConfig = serde_json::from_str(value)
                    .map_err(|_| Error::InvalidArgument("Invalid JSON for env"))?;
                *options = std::mem::take(options).env(&env.name, &env.value);
            }
            "mount" => {
                let mount: MountConfig = serde_json::from_str(value)
                    .map_err(|_| Error::InvalidArgument("Invalid JSON for mount"))?;
                *options = std::mem::take(options).mount(
                    &mount.host,
                    &mount.guest,
                    mount.dir_perms(),
                    mount.file_perms(),
                );
            }
            _ => return Err(Error::InvalidArgument("Unknown config key")),
        }
        Ok(())
    }

    fn set_handler(&self, handler: Arc<SandboxHandler>) -> Result<()> {
        self.handler_slot
            .set(handler)
            .map_err(|_| Error::InvalidArgument("Handler already set"))
    }

    fn start(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.inner, SandboxInner::Uninitialized) {
            SandboxInner::Pending { options } => {
                let handler = self
                    .handler_slot
                    .get()
                    .ok_or(Error::InvalidArgument("Handler not set"))?
                    .clone();

                let module = self
                    .ctx
                    .module
                    .as_ref()
                    .ok_or(Error::InvalidArgument("Runtime not loaded"))?;
                let env = Env::new(self.handler_slot.clone());
                let sandbox = self
                    .ctx
                    .rt
                    .block_on(async { module.instantiate(env, options.clone()).await });
                let sandbox = match sandbox {
                    Ok(sandbox) => sandbox,
                    Err(error) => {
                        self.inner = SandboxInner::Pending { options };
                        return Err(Error::Internal(format!(
                            "Failed to create instance: {error}"
                        )));
                    }
                };

                self.inner = SandboxInner::Running { sandbox, handler };
                Ok(())
            }
            _ => Err(Error::InvalidArgument("Instance not in pending state")),
        }
    }

    fn load_script(&mut self, input: &str, timeout_in_ms: u64) -> Result<()> {
        match &mut self.inner {
            SandboxInner::Running { sandbox, handler } => {
                self.ctx
                    .rt
                    .block_on(async {
                        tokio::time::timeout(
                            Duration::from_millis(timeout_in_ms),
                            sandbox.eval_script(input, handler.clone()),
                        )
                        .await
                    })
                    .map_err(|_| Error::Internal("Script execution timeout".to_string()))?
                    .map_err(|e| Error::Internal(format!("Script loading failed: {e}")))?;

                Ok(())
            }
            _ => Err(Error::InvalidArgument("Instance not running")),
        }
    }

    fn run(&mut self, func: &str, args: Vec<RawArgument>, timeout_in_ms: u64) -> Result<()> {
        let isola_args: Vec<_> = args
            .into_iter()
            .map(|arg| match arg {
                RawArgument::Value(name, value) => match name {
                    Some(name) => Arg::Named(name, value),
                    None => Arg::Positional(value),
                },
                RawArgument::Stream(name, receiver) => {
                    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver));
                    match name {
                        Some(name) => Arg::NamedStream(name, stream),
                        None => Arg::PositionalStream(stream),
                    }
                }
            })
            .collect();

        match std::mem::replace(&mut self.inner, SandboxInner::Uninitialized) {
            SandboxInner::Running {
                mut sandbox,
                handler,
            } => {
                let timeout = Duration::from_millis(timeout_in_ms);
                let result = self.ctx.rt.block_on(async {
                    tokio::time::timeout(
                        timeout,
                        sandbox.call_with_sink(func, isola_args, handler.clone()),
                    )
                    .await
                });

                // Restore the sandbox state.
                self.inner = SandboxInner::Running { sandbox, handler };

                result.map_or_else(
                    |_| {
                        Err(Error::Internal(format!(
                            "Sandbox execution timed out after {}ms",
                            timeout.as_millis()
                        )))
                    },
                    |inner| {
                        inner.map_err(|e| Error::Internal(format!("Sandbox execution failed: {e}")))
                    },
                )?;

                Ok(())
            }
            _ => Err(Error::InvalidArgument("Instance not running")),
        }
    }
}

/// Creates a new sandbox instance from the context.
///
/// # Safety
///
/// `ctx` must be a live context handle and `out_sandbox` must point to writable
/// handle storage. The output is set to `NULL` before creation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_create(
    ctx: *const ContextHandle,
    out_sandbox: *mut *mut SandboxHandle,
) -> ErrorCode {
    let ctx = c_try!(unsafe { require_ref(ctx, "ctx must not be NULL") });
    let out_sandbox = c_try!(unsafe { require_mut(out_sandbox, "out_sandbox must not be NULL") });
    *out_sandbox = std::ptr::null_mut();
    let sandbox = c_try!(ctx.0.new_sandbox());
    *out_sandbox = Box::into_raw(Box::new(sandbox));
    ErrorCode::Ok
}

/// Destroy a sandbox handle. Passing `NULL` is allowed and has no effect.
///
/// # Safety
/// A non-`NULL` handle must have been returned by `isola_sandbox_create` and
/// must not have been destroyed previously.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_destroy(sandbox: *mut SandboxHandle) {
    if !sandbox.is_null() {
        drop(unsafe { Box::from_raw(sandbox) });
    }
}

/// Sets a per-sandbox configuration value, overriding context-level defaults.
///
/// Must be called **after** `isola_sandbox_create` and **before**
/// `isola_sandbox_start`. Returns `ISOLA_ERROR_CODE_INVALID_ARGUMENT` if the
/// sandbox has already been started or if `key` is unrecognized.
///
/// # Supported keys
///
/// | Key          | Value                                                | Default         |
/// |--------------|------------------------------------------------------|-----------------|
/// | `max_memory` | Decimal byte count (e.g. `"33554432"` for 32 MiB).  | *(from context)*|
/// | `env`        | JSON: `{"name":"VAR","value":"val"}`                 | *(none)*        |
/// | `mount`      | JSON: `{"host":"/h","guest":"/g","writable":false}`  | *(none)*        |
///
/// The `env` and `mount` keys may be called multiple times. Per-sandbox
/// settings are merged with context-level defaults: `max_memory` replaces,
/// `env` overrides by key, and `mount` overrides by guest path.
///
/// # Safety
///
/// The caller must ensure that both `key` and `value` are valid,
/// null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_set_config(
    sandbox: *mut SandboxHandle,
    key: *const c_char,
    value: *const c_char,
) -> ErrorCode {
    let sandbox = c_try!(unsafe { require_mut(sandbox, "sandbox must not be NULL") });
    let key = c_try!(unsafe { require_cstr(key, "key must not be NULL") });
    let value = c_try!(unsafe { require_cstr(value, "value must not be NULL") });
    c_try!(sandbox.set_config(key, value));
    ErrorCode::Ok
}

#[repr(C)]
pub enum CallbackEvent {
    ResultJson = 0,
    EndJson = 4,
    Stdout = 1,
    Stderr = 2,
    Error = 3,
    Log = 5,
}

/// Sets the handler vtable on a sandbox.
///
/// The vtable is copied; the caller may free it after this call returns.
/// Must be called exactly once, before `isola_sandbox_start`.
///
/// # Safety
///
/// `vtable` must point to a valid `SandboxHandlerVtable`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_set_handler(
    sandbox: *mut SandboxHandle,
    vtable: *const SandboxHandlerVtable,
    user_data: *mut c_void,
) -> ErrorCode {
    let sandbox = c_try!(unsafe { require_mut(sandbox, "sandbox must not be NULL") });
    let vtable = c_try!(unsafe { require_ref(vtable, "vtable must not be NULL") });
    let Some(on_event) = vtable.on_event else {
        return fail(Error::InvalidArgument("vtable.on_event must not be NULL"));
    };
    let vtable = SandboxHandlerVtable {
        on_event: vtable.on_event,
        http_request: vtable.http_request,
        hostcall: vtable.hostcall,
    };
    let handler = Arc::new(SandboxHandler {
        vtable,
        on_event,
        user_data,
    });
    c_try!(sandbox.set_handler(handler));
    ErrorCode::Ok
}

/// Start a configured sandbox.
///
/// # Safety
/// `sandbox` must be a live handle returned by `isola_sandbox_create`. `NULL`
/// is rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_start(sandbox: *mut SandboxHandle) -> ErrorCode {
    let sandbox = c_try!(unsafe { require_mut(sandbox, "sandbox must not be NULL") });
    c_try!(sandbox.start());
    ErrorCode::Ok
}

/// Loads a script into the sandbox.
///
/// # Safety
///
/// The caller must ensure that `input` is a valid, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_load_script(
    sandbox: *mut SandboxHandle,
    input: *const c_char,
    timeout_in_ms: u64,
) -> ErrorCode {
    let sandbox = c_try!(unsafe { require_mut(sandbox, "sandbox must not be NULL") });
    let input = c_try!(unsafe { require_cstr(input, "input must not be NULL") });
    let input = c_try!(
        input
            .to_str()
            .map_or_else(|_| Err(Error::InvalidArgument("Invalid input string")), Ok)
    );
    c_try!(sandbox.load_script(input, timeout_in_ms));
    ErrorCode::Ok
}

/// Runs a function in the sandbox with the specified arguments.
///
/// # Safety
///
/// The caller must ensure that:
/// - `func` is a valid, null-terminated C string
/// - `args` is a valid pointer to an array of `Argument` structs of length
///   `args_len`
/// - Each `Argument` in the array has valid pointers and data
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_run(
    sandbox: *mut SandboxHandle,
    func: *const c_char,
    args: *const Argument,
    args_len: usize,
    timeout_in_ms: u64,
) -> ErrorCode {
    let sandbox = c_try!(unsafe { require_mut(sandbox, "sandbox must not be NULL") });
    let func = c_try!(unsafe { require_cstr(func, "func must not be NULL") });
    let func = c_try!(
        func.to_str()
            .map_err(|_| Error::InvalidArgument("Invalid function name"))
    );

    let mut validated_args = Vec::with_capacity(args_len);
    let mut unique_stream_ptrs = std::collections::HashSet::new();
    let mut stream_ptrs = Vec::new();
    if args_len != 0 {
        if args.is_null() {
            return fail(Error::InvalidArgument(
                "args must not be NULL when args_len is non-zero",
            ));
        }
        for arg in unsafe { std::slice::from_raw_parts(args, args_len) } {
            let name = if arg.name.is_null() {
                None
            } else {
                let name = unsafe { CStr::from_ptr(arg.name) };
                let name = c_try!(
                    name.to_str()
                        .map_err(|_| Error::InvalidArgument("Invalid argument name"))
                );
                Some(name.to_string())
            };

            let validated_arg = match arg.kind {
                ISOLA_ARGUMENT_KIND_VALUE => {
                    let encoded = unsafe { arg.value.value };
                    match c_try!(parse_argument_type(encoded.format)) {
                        ValueFormat::Json => {
                            let value = c_try!(
                                unsafe { blob_as_slice(encoded.data) }
                                    .ok_or(Error::InvalidArgument("Invalid JSON argument buffer"))
                            );
                            let json = c_try!(std::str::from_utf8(value).map_err(|_| {
                                Error::InvalidArgument("Invalid UTF-8 in JSON argument")
                            }));
                            let value = c_try!(
                                Value::from_json_str(json)
                                    .map_err(|_| Error::InvalidArgument("Invalid JSON argument"))
                            );
                            ValidatedArgument::Value(name, value)
                        }
                        ValueFormat::Cbor => {
                            let value = c_try!(
                                unsafe { blob_as_slice(encoded.data) }
                                    .ok_or(Error::InvalidArgument("Invalid CBOR argument buffer"))
                            );
                            ValidatedArgument::Value(name, Value::from_cbor(value.to_vec()))
                        }
                    }
                }
                ISOLA_ARGUMENT_KIND_STREAM => {
                    let stream_ptr = unsafe { arg.value.stream };
                    if stream_ptr.is_null() {
                        return fail(Error::InvalidArgument(
                            "Stream argument handle must not be NULL",
                        ));
                    }
                    if !unique_stream_ptrs.insert(stream_ptr) {
                        return fail(Error::InvalidArgument(
                            "Stream argument handle must not be reused",
                        ));
                    }
                    let stream_index = stream_ptrs.len();
                    stream_ptrs.push(stream_ptr);
                    ValidatedArgument::Stream(name, stream_index)
                }
                _ => return fail(Error::InvalidArgument("Unknown argument kind")),
            };
            validated_args.push(validated_arg);
        }
    }

    let mut receivers = c_try!(take_stream_receivers(&stream_ptrs));

    let mut parsed_args = Vec::with_capacity(validated_args.len());
    for arg in validated_args {
        let parsed = match arg {
            ValidatedArgument::Value(name, value) => RawArgument::Value(name, value),
            ValidatedArgument::Stream(name, stream_index) => {
                let Some(receiver) = receivers.get_mut(stream_index).and_then(Option::take) else {
                    crate::error::set_last_error(Error::Internal(
                        "validated stream receiver was not acquired".to_string(),
                    ));
                    return ErrorCode::Internal;
                };
                RawArgument::Stream(name, receiver)
            }
        };
        parsed_args.push(parsed);
    }

    c_try!(sandbox.run(func, parsed_args, timeout_in_ms));
    ErrorCode::Ok
}

// ---------------------------------------------------------------------------
// Argument types
// ---------------------------------------------------------------------------

/// Stable-width wire value describing the encoding of an argument.
///
/// Additional values may be added in future releases. Unknown values are
/// rejected by the receiving runtime.
pub type ArgumentType = i32;
pub const ISOLA_ARGUMENT_TYPE_JSON: ArgumentType = 0;
pub const ISOLA_ARGUMENT_TYPE_CBOR: ArgumentType = 1;

/// Stable-width wire value describing an argument's storage kind.
///
/// Additional values may be added in future releases. Unknown values are
/// rejected by the receiving runtime.
pub type ArgumentKind = i32;
pub const ISOLA_ARGUMENT_KIND_VALUE: ArgumentKind = 0;
pub const ISOLA_ARGUMENT_KIND_STREAM: ArgumentKind = 1;

#[derive(Clone, Copy)]
enum ValueFormat {
    Json,
    Cbor,
}

const fn parse_argument_type(value: ArgumentType) -> Result<ValueFormat> {
    match value {
        ISOLA_ARGUMENT_TYPE_JSON => Ok(ValueFormat::Json),
        ISOLA_ARGUMENT_TYPE_CBOR => Ok(ValueFormat::Cbor),
        _ => Err(Error::InvalidArgument("Unknown argument format")),
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Blob {
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct EncodedValue {
    pub format: ArgumentType,
    pub data: Blob,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union ArgumentValue {
    pub value: EncodedValue,
    pub stream: *const StreamHandle,
}

#[repr(C)]
pub struct Argument {
    pub kind: ArgumentKind,
    pub name: *const c_char,
    pub value: ArgumentValue,
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

pub struct StreamHandle {
    format: ValueFormat,
    sender: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<Value>>>,
    receiver: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Value>>>,
}

impl StreamHandle {
    fn take_receiver(&self) -> Result<tokio::sync::mpsc::Receiver<Value>> {
        self.receiver
            .lock()
            .map_err(|_| Error::Internal("Stream receiver mutex poisoned".to_string()))?
            .take()
            .ok_or(Error::InvalidArgument("Stream receiver already taken"))
    }

    fn restore_receiver(&self, receiver: tokio::sync::mpsc::Receiver<Value>) {
        let Ok(mut slot) = self.receiver.lock() else {
            return;
        };
        debug_assert!(slot.is_none());
        *slot = Some(receiver);
    }
}

enum RawArgument {
    Value(Option<String>, Value),
    Stream(Option<String>, tokio::sync::mpsc::Receiver<Value>),
}

enum ValidatedArgument {
    Value(Option<String>, Value),
    Stream(Option<String>, usize),
}

unsafe fn blob_as_slice<'a>(blob: Blob) -> Option<&'a [u8]> {
    if blob.len == 0 {
        return Some(&[]);
    }
    (!blob.data.is_null()).then(|| unsafe { std::slice::from_raw_parts(blob.data, blob.len) })
}

fn take_stream_receivers(
    stream_ptrs: &[*const StreamHandle],
) -> Result<Vec<Option<tokio::sync::mpsc::Receiver<Value>>>> {
    let mut receivers = Vec::with_capacity(stream_ptrs.len());
    for stream_ptr in stream_ptrs {
        let stream = unsafe { &**stream_ptr };
        let receiver = match stream.take_receiver() {
            Ok(receiver) => receiver,
            Err(error) => {
                for (acquired_ptr, receiver) in stream_ptrs.iter().zip(receivers).rev() {
                    if let Some(receiver) = receiver {
                        unsafe { &**acquired_ptr }.restore_receiver(receiver);
                    }
                }
                return Err(error);
            }
        };
        receivers.push(Some(receiver));
    }
    Ok(receivers)
}

/// Creates a new stream handle for streaming arguments.
///
/// # Safety
///
/// `out_stream` must point to writable handle storage. The output is set to
/// `NULL` before creation. Unknown `format` values are rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_stream_create(
    format: ArgumentType,
    out_stream: *mut *mut StreamHandle,
) -> ErrorCode {
    let out_stream = c_try!(unsafe { require_mut(out_stream, "out_stream must not be NULL") });
    *out_stream = std::ptr::null_mut();
    let format = c_try!(parse_argument_type(format));
    let (sender, receiver) = tokio::sync::mpsc::channel(1024);
    let stream = Box::new(StreamHandle {
        format,
        sender: std::sync::Mutex::new(Some(sender)),
        receiver: std::sync::Mutex::new(Some(receiver)),
    });
    *out_stream = Box::into_raw(stream);
    ErrorCode::Ok
}

/// Pushes data to a stream.
///
/// # Safety
///
/// The caller must ensure that `data` points to a valid buffer of length `len`.
///
/// # Parameters
///
/// * `blocking` - If non-zero, blocks until space is available in the channel.
///   If zero, returns immediately with an error if the channel is full.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_stream_push(
    stream: *const StreamHandle,
    data: *const u8,
    len: usize,
    blocking: c_int,
) -> ErrorCode {
    let stream = c_try!(unsafe { require_ref(stream, "stream must not be NULL") });
    let Some(data) = (unsafe { blob_as_slice(Blob { data, len }) }) else {
        let err = Error::InvalidArgument("Invalid stream buffer");
        crate::error::set_last_error(err);
        return ErrorCode::InvalidArgument;
    };
    let value = match stream.format {
        ValueFormat::Json => {
            let Ok(json) = std::str::from_utf8(data) else {
                let err = Error::InvalidArgument("Invalid UTF-8 in stream value");
                crate::error::set_last_error(err);
                return ErrorCode::InvalidArgument;
            };
            let Ok(value) = Value::from_json_str(json) else {
                let err = Error::InvalidArgument("Invalid JSON in stream value");
                crate::error::set_last_error(err);
                return ErrorCode::InvalidArgument;
            };
            value
        }
        ValueFormat::Cbor => Value::from_cbor(data.to_vec()),
    };

    send_stream_value(stream, value, blocking)
}

fn send_stream_value(stream: &StreamHandle, value: Value, blocking: c_int) -> ErrorCode {
    if blocking != 0 {
        // Clone the sender so the mutex is not held while blocking.
        let sender = match stream.sender.lock() {
            Ok(sender) => sender.as_ref().cloned(),
            Err(_) => return fail(Error::Internal("Stream mutex poisoned".to_string())),
        };
        let Some(sender) = sender else {
            return fail(Error::StreamClosed);
        };
        if sender.blocking_send(value).is_ok() {
            ErrorCode::Ok
        } else {
            fail(Error::StreamClosed)
        }
    } else {
        let Ok(sender_guard) = stream.sender.lock() else {
            return fail(Error::Internal("Stream mutex poisoned".to_string()));
        };
        let Some(sender) = sender_guard.as_ref() else {
            return fail(Error::StreamClosed);
        };
        let result = sender.try_send(value);
        drop(sender_guard);
        match result {
            Ok(()) => ErrorCode::Ok,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => fail(Error::StreamFull),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => fail(Error::StreamClosed),
        }
    }
}

/// Signals the end of a stream and frees the handle.
///
/// After calling this function, no more data can be pushed to the stream
/// and the handle is invalid.
///
/// # Safety
/// `stream` must be a live handle returned by `isola_stream_create` that has
/// not already been ended. `NULL` is rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_stream_end(stream: *mut StreamHandle) -> ErrorCode {
    let stream = c_try!(unsafe { require_mut(stream, "stream must not be NULL") });
    // Dropping the StreamHandle closes the sender and frees the handle.
    drop(unsafe { Box::from_raw(std::ptr::from_mut(stream)) });
    ErrorCode::Ok
}

// ---------------------------------------------------------------------------
// HTTP response body (push-based)
// ---------------------------------------------------------------------------

/// Delivers the HTTP status code and response headers.
///
/// Must be called exactly once per handle, before any
/// `isola_http_response_body_push` calls.
///
/// The header data is copied; the caller may free the array after this
/// call returns.
///
/// # Safety
///
/// - `body` must be a live handle obtained from an `http_request` callback.
/// - `headers` must point to a valid array of `headers_len` elements (may be
///   NULL if `headers_len` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_http_response_body_start(
    body: *const crate::env::HttpResponseBody,
    status: u16,
    headers: *const crate::env::HttpHeader,
    headers_len: usize,
) -> ErrorCode {
    let body = c_try!(unsafe { require_ref(body, "body must not be NULL") });
    if http::StatusCode::from_u16(status).is_err() {
        return fail(Error::InvalidArgument("Invalid HTTP response status"));
    }
    let owned_headers = if headers_len == 0 {
        Vec::new()
    } else {
        if headers.is_null() {
            return fail(Error::InvalidArgument(
                "headers must not be NULL when headers_len is non-zero",
            ));
        }
        let Some(headers) = (unsafe { std::slice::from_raw_parts(headers, headers_len) })
            .iter()
            .map(|h| unsafe {
                Some((
                    blob_as_slice(Blob {
                        data: h.name,
                        len: h.name_len,
                    })?
                    .to_vec(),
                    blob_as_slice(Blob {
                        data: h.value,
                        len: h.value_len,
                    })?
                    .to_vec(),
                ))
            })
            .collect::<Option<Vec<_>>>()
        else {
            crate::error::set_last_error(Error::InvalidArgument("Invalid HTTP header buffer"));
            return ErrorCode::InvalidArgument;
        };
        headers
    };

    let head = crate::env::HttpResponseHead {
        status,
        headers: owned_headers,
    };

    if body.start(head).is_err() {
        let err = Error::Internal("response already started or receiver dropped".to_string());
        crate::error::set_last_error(err);
        return ErrorCode::Internal;
    }
    ErrorCode::Ok
}

/// Pushes a chunk of response body data.
///
/// May be called from any thread. Blocks if the internal channel is full.
///
/// # Safety
///
/// - `body` must be a live handle obtained from an `http_request` callback.
/// - `data` must point to a valid buffer of `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_http_response_body_push(
    body: *const crate::env::HttpResponseBody,
    data: *const u8,
    len: usize,
) -> ErrorCode {
    let body = c_try!(unsafe { require_ref(body, "body must not be NULL") });
    let Some(data) = (unsafe { blob_as_slice(Blob { data, len }) }) else {
        return fail(Error::InvalidArgument("Invalid HTTP response body buffer"));
    };
    let chunk = bytes::Bytes::copy_from_slice(data);
    if body.send(chunk).is_err() {
        let err = Error::StreamClosed;
        crate::error::set_last_error(err);
        return ErrorCode::StreamClosed;
    }
    ErrorCode::Ok
}

/// Signals EOF and frees the response body handle.
///
/// Must be called exactly once per handle, even if no data was pushed.
///
/// # Safety
///
/// `body` must be `NULL` or a live handle obtained from an `http_request`
/// callback. After this call a non-`NULL` pointer is invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_http_response_body_close(body: *mut crate::env::HttpResponseBody) {
    // Dropping the sender signals EOF to the receiver stream.
    if !body.is_null() {
        drop(unsafe { Box::from_raw(body) });
    }
}

// ---------------------------------------------------------------------------
// Hostcall response (non-blocking)
// ---------------------------------------------------------------------------

/// Resolves a hostcall with a JSON result value, consuming the handle.
///
/// Must be called exactly once per handle. After this call the pointer is
/// invalid. Use `isola_hostcall_response_reject` to deliver an error instead.
///
/// If the data is not valid JSON, the handle is **not** consumed and the
/// caller may retry with corrected data or call
/// `isola_hostcall_response_reject`.
///
/// # Safety
///
/// - `response` must be a live handle obtained from a `hostcall` callback.
/// - `data` must point to a valid JSON buffer of `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_hostcall_response_resolve(
    response: *mut crate::env::HostcallResponse,
    data: *const u8,
    len: usize,
) -> ErrorCode {
    if response.is_null() {
        return fail(Error::InvalidArgument("response must not be NULL"));
    }
    // Validate before consuming so the handle survives parse errors.
    let Some(json) = (unsafe { blob_as_slice(Blob { data, len }) }) else {
        return fail(Error::InvalidArgument("Invalid hostcall response buffer"));
    };
    let Ok(json_str) = std::str::from_utf8(json) else {
        crate::error::set_last_error(Error::InvalidArgument("Invalid UTF-8 in hostcall response"));
        return ErrorCode::InvalidArgument;
    };
    let Ok(value) = Value::from_json_str(json_str) else {
        crate::error::set_last_error(Error::InvalidArgument("Invalid JSON in hostcall response"));
        return ErrorCode::InvalidArgument;
    };
    // Now consume the handle.
    let response = unsafe { Box::from_raw(response) };
    if response.resolve(value).is_err() {
        crate::error::set_last_error(Error::Internal(
            "hostcall response already completed or receiver dropped".to_string(),
        ));
        return ErrorCode::Internal;
    }
    ErrorCode::Ok
}

/// Rejects a hostcall with an error message, consuming the handle.
///
/// Must be called exactly once per handle. After this call the pointer is
/// invalid. Use `isola_hostcall_response_resolve` to deliver a result instead.
///
/// # Safety
///
/// - `response` must be a live handle obtained from a `hostcall` callback.
/// - `error_message` must be a valid, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_hostcall_response_reject(
    response: *mut crate::env::HostcallResponse,
    error_message: *const c_char,
) -> ErrorCode {
    if response.is_null() {
        return fail(Error::InvalidArgument("response must not be NULL"));
    }
    let msg = c_try!(unsafe { require_cstr(error_message, "error_message must not be NULL") });
    let Ok(msg_str) = msg.to_str() else {
        crate::error::set_last_error(Error::InvalidArgument("Invalid UTF-8 in error message"));
        return ErrorCode::InvalidArgument;
    };
    let response = unsafe { Box::from_raw(response) };
    if response.reject(msg_str.to_string()).is_err() {
        crate::error::set_last_error(Error::Internal(
            "hostcall response already completed or receiver dropped".to_string(),
        ));
        return ErrorCode::Internal;
    }
    ErrorCode::Ok
}

/// Cancels a hostcall without a result, consuming the handle.
///
/// Use this when external work is abandoned. The waiting guest call fails and
/// the response allocation is released.
///
/// # Safety
///
/// `response` must be `NULL` or a live handle obtained from a `hostcall`
/// callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_hostcall_response_cancel(
    response: *mut crate::env::HostcallResponse,
) {
    if !response.is_null() {
        drop(unsafe { Box::from_raw(response) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_rejects_null_handles_and_output_slots() {
        assert_eq!(
            unsafe { isola_context_create(0, std::ptr::null_mut()) },
            ErrorCode::InvalidArgument
        );
        assert!(!crate::error::isola_last_error().is_null());

        assert_eq!(
            unsafe { isola_context_initialize(std::ptr::null_mut(), std::ptr::null()) },
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            unsafe { isola_sandbox_create(std::ptr::null(), std::ptr::null_mut()) },
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            unsafe { isola_sandbox_start(std::ptr::null_mut()) },
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            unsafe { isola_stream_push(std::ptr::null(), std::ptr::null(), 0, 0) },
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            unsafe { isola_stream_end(std::ptr::null_mut()) },
            ErrorCode::InvalidArgument
        );

        unsafe {
            isola_context_destroy(std::ptr::null_mut());
            isola_sandbox_destroy(std::ptr::null_mut());
            isola_http_response_body_close(std::ptr::null_mut());
            isola_hostcall_response_cancel(std::ptr::null_mut());
        }
    }

    #[test]
    fn ffi_rejects_unknown_stream_format_without_allocating() {
        let mut stream = std::ptr::dangling_mut();
        assert_eq!(
            unsafe { isola_stream_create(99, &raw mut stream) },
            ErrorCode::InvalidArgument
        );
        assert!(stream.is_null());
    }

    #[test]
    fn invalid_value_does_not_consume_stream_receiver() {
        let context = ContextCore::create_handle(0).expect("create context");
        let mut sandbox = SandboxHandle {
            ctx: Arc::clone(&context.0),
            handler_slot: Arc::new(OnceLock::new()),
            inner: SandboxInner::Pending {
                options: SandboxOptions::default(),
            },
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let stream = StreamHandle {
            format: ValueFormat::Json,
            sender: std::sync::Mutex::new(Some(sender)),
            receiver: std::sync::Mutex::new(Some(receiver)),
        };
        let invalid_json = b"{";
        let args = [
            Argument {
                kind: ISOLA_ARGUMENT_KIND_STREAM,
                name: std::ptr::null(),
                value: ArgumentValue {
                    stream: std::ptr::from_ref(&stream),
                },
            },
            Argument {
                kind: ISOLA_ARGUMENT_KIND_VALUE,
                name: std::ptr::null(),
                value: ArgumentValue {
                    value: EncodedValue {
                        format: ISOLA_ARGUMENT_TYPE_JSON,
                        data: Blob {
                            data: invalid_json.as_ptr(),
                            len: invalid_json.len(),
                        },
                    },
                },
            },
        ];

        assert_eq!(
            unsafe {
                isola_sandbox_run(
                    &raw mut sandbox,
                    c"main".as_ptr(),
                    args.as_ptr(),
                    args.len(),
                    1000,
                )
            },
            ErrorCode::InvalidArgument
        );
        assert!(stream.take_receiver().is_ok());
    }

    #[test]
    fn failed_stream_acquisition_restores_other_receivers() {
        let (first_sender, first_receiver) = tokio::sync::mpsc::channel(1);
        let first = StreamHandle {
            format: ValueFormat::Json,
            sender: std::sync::Mutex::new(Some(first_sender)),
            receiver: std::sync::Mutex::new(Some(first_receiver)),
        };
        let (second_sender, second_receiver) = tokio::sync::mpsc::channel(1);
        let second = StreamHandle {
            format: ValueFormat::Json,
            sender: std::sync::Mutex::new(Some(second_sender)),
            receiver: std::sync::Mutex::new(Some(second_receiver)),
        };
        let held_receiver = second.take_receiver().expect("take second receiver");
        let streams = [std::ptr::from_ref(&first), std::ptr::from_ref(&second)];

        assert!(take_stream_receivers(&streams).is_err());
        assert!(first.take_receiver().is_ok());
        drop(held_receiver);
    }
}
