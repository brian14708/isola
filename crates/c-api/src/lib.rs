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

// ---------------------------------------------------------------------------
// Unified sandbox handler vtable
// ---------------------------------------------------------------------------

/// Unified vtable for sandbox event handling and optional HTTP support.
///
/// - `on_event` is required.
/// - `http_request` is optional (NULL to disable HTTP).
#[repr(C)]
pub struct SandboxHandlerVtable {
    /// Called for output events (results, logs, stdout/stderr).
    pub on_event: extern "C" fn(CallbackEvent, *const u8, usize, *mut c_void),

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
        ) -> ErrorCode,
    >,
}

/// Resolved handler: vtable + `user_data`, stored internally.
pub struct SandboxHandler {
    pub vtable: SandboxHandlerVtable,
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
        (self.vtable.on_event)(
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
            (self.vtable.on_event)(
                CallbackEvent::EndJson,
                data.as_ptr(),
                data.len(),
                self.user_data,
            );
        } else {
            (self.vtable.on_event)(CallbackEvent::EndJson, std::ptr::null(), 0, self.user_data);
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
        (self.vtable.on_event)(event, message.as_ptr(), message.len(), self.user_data);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

pub struct ContextHandle {
    rt: Runtime,
    module: Option<SandboxTemplate<Env>>,
}

impl ContextHandle {
    fn new(nr_thread: i32) -> Result<Box<Self>> {
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
        Ok(Box::new(Self { rt, module: None }))
    }

    fn set_config(&self, _key: &CStr, _value: &CStr) -> Result<()> {
        _ = self;
        todo!();
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

        self.rt.block_on(async {
            let module = SandboxTemplate::<Env>::builder()
                .prelude(Some("import sandbox.asyncio".to_string()))
                .cache(Some(parent.join("cache")))
                .max_memory(64 * 1024 * 1024)
                .mount(&lib_dir, "/lib", DirPerms::READ, FilePerms::READ)
                .build(&path)
                .await
                .map_err(|e| Error::Internal(format!("Failed to load runtime: {e}")))?;
            self.module = Some(module);
            Ok(())
        })
    }

    fn new_sandbox(&self) -> Result<SandboxHandle<'_>> {
        let Some(module) = &self.module else {
            return Err(Error::InvalidArgument("Runtime not loaded"));
        };
        let handler_slot = Arc::new(OnceLock::new());
        let env = Env::new(handler_slot.clone());
        let sandbox = self
            .rt
            .block_on(async { module.instantiate(env, SandboxOptions::default()).await })
            .map_err(|e| Error::Internal(format!("Failed to create instance: {e}")))?;
        Ok(SandboxHandle {
            ctx: self,
            handler_slot,
            inner: SandboxInner::Pending { sandbox },
        })
    }
}

/// Creates a new isola context with the specified number of threads.
///
/// # Safety
///
/// The caller must ensure that `out_context` is a valid pointer to an
/// uninitialized `Box<ContextHandle>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_create(
    nr_thread: c_int,
    out_context: *mut Box<ContextHandle>,
) -> ErrorCode {
    let ctx = c_try!(ContextHandle::new(nr_thread));
    unsafe { out_context.write(ctx) };
    ErrorCode::Ok
}

/// Initializes the isola context with the specified path.
///
/// # Safety
///
/// The caller must ensure that `path` is a valid, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_initialize(
    ctx: &mut ContextHandle,
    path: *const c_char,
) -> ErrorCode {
    let path = unsafe { CStr::from_ptr(path) };
    let path = c_try!(
        path.to_str()
            .map_or_else(|_| Err(Error::InvalidArgument("Invalid path string")), Ok)
    );
    c_try!(ctx.load(path));
    ErrorCode::Ok
}

/// Sets a configuration value for the isola context.
///
/// # Safety
///
/// The caller must ensure that both `key` and `value` are valid,
/// null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_context_config_set(
    ctx: &mut ContextHandle,
    key: *const c_char,
    value: *const c_char,
) -> ErrorCode {
    let key = unsafe { CStr::from_ptr(key) };
    let value = unsafe { CStr::from_ptr(value) };
    c_try!(ctx.set_config(key, value));
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn isola_context_destroy(_ctx: Box<ContextHandle>) {}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

enum SandboxInner {
    Uninitialized,
    Pending {
        sandbox: Sandbox<Env>,
    },
    Running {
        sandbox: Sandbox<Env>,
        handler: Arc<SandboxHandler>,
    },
}

pub struct SandboxHandle<'a> {
    ctx: &'a ContextHandle,
    handler_slot: Arc<OnceLock<Arc<SandboxHandler>>>,
    inner: SandboxInner,
}

impl SandboxHandle<'_> {
    fn set_config(&self, _key: &CStr, _value: &CStr) -> Result<()> {
        todo!()
    }

    fn set_handler(&self, handler: Arc<SandboxHandler>) -> Result<()> {
        self.handler_slot
            .set(handler)
            .map_err(|_| Error::InvalidArgument("Handler already set"))
    }

    fn start(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.inner, SandboxInner::Uninitialized) {
            SandboxInner::Pending { sandbox } => {
                let handler = self
                    .handler_slot
                    .get()
                    .ok_or(Error::InvalidArgument("Handler not set"))?
                    .clone();
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
        match std::mem::replace(&mut self.inner, SandboxInner::Uninitialized) {
            SandboxInner::Running {
                mut sandbox,
                handler,
            } => {
                // Convert arguments to isola format.
                let isola_args: Vec<_> = args
                    .into_iter()
                    .map(|arg| -> Result<Arg> {
                        match arg {
                            RawArgument::Json(name, value) => {
                                let json_str = std::str::from_utf8(&value).map_err(|_| {
                                    Error::InvalidArgument("Invalid UTF-8 in JSON argument")
                                })?;
                                let value = Value::from_json_str(json_str)
                                    .map_err(|_| Error::InvalidArgument("Invalid JSON format"))?;
                                Ok(match name {
                                    Some(name) => Arg::Named(name, value),
                                    None => Arg::Positional(value),
                                })
                            }
                            RawArgument::JsonStream(name, receiver) => {
                                let stream =
                                    Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver));
                                Ok(match name {
                                    Some(name) => Arg::NamedStream(name, stream),
                                    None => Arg::PositionalStream(stream),
                                })
                            }
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;

                let func = func.to_string();
                let timeout = Duration::from_millis(timeout_in_ms);
                let result = self.ctx.rt.block_on(async {
                    tokio::time::timeout(
                        timeout,
                        sandbox.call_with_sink(&func, isola_args, handler.clone()),
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
/// The caller must ensure that `out_sandbox` is a valid pointer to an
/// uninitialized `Box<SandboxHandle>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_create<'a>(
    ctx: &'a mut ContextHandle,
    out_sandbox: *mut Box<SandboxHandle<'a>>,
) -> ErrorCode {
    let sandbox = c_try!(ctx.new_sandbox());
    unsafe { out_sandbox.write(Box::new(sandbox)) };
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn isola_sandbox_destroy(_sandbox: Box<SandboxHandle<'_>>) {}

/// Sets a configuration value for the sandbox.
///
/// # Safety
///
/// The caller must ensure that both `key` and `value` are valid,
/// null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_set_config(
    sandbox: &mut SandboxHandle<'_>,
    key: *const c_char,
    value: *const c_char,
) -> ErrorCode {
    let key = unsafe { CStr::from_ptr(key) };
    let value = unsafe { CStr::from_ptr(value) };
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
    sandbox: &mut SandboxHandle<'_>,
    vtable: *const SandboxHandlerVtable,
    user_data: *mut c_void,
) -> ErrorCode {
    let vtable = unsafe { std::ptr::read(vtable) };
    let handler = Arc::new(SandboxHandler { vtable, user_data });
    c_try!(sandbox.set_handler(handler));
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn isola_sandbox_start(sandbox: &mut SandboxHandle<'_>) -> ErrorCode {
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
    sandbox: &mut SandboxHandle<'_>,
    input: *const c_char,
    timeout_in_ms: u64,
) -> ErrorCode {
    let input = unsafe { CStr::from_ptr(input) };
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
///
/// # Panics
///
/// This function may panic if argument names contain invalid UTF-8 sequences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_sandbox_run(
    sandbox: &mut SandboxHandle<'_>,
    func: *const c_char,
    args: *const Argument,
    args_len: usize,
    timeout_in_ms: u64,
) -> ErrorCode {
    let args = if args_len == 0 {
        vec![]
    } else {
        {
            let mut parsed_args = Vec::new();
            for arg in unsafe { std::slice::from_raw_parts(args, args_len) } {
                let name = if arg.name.is_null() {
                    None
                } else {
                    let name = unsafe { CStr::from_ptr(arg.name) };
                    match name.to_str() {
                        Ok(s) => Some(s.to_string()),
                        Err(_) => return ErrorCode::InvalidArgument,
                    }
                };

                let parsed_arg = match arg.r#type {
                    ArgumentType::Json => {
                        let json_data = unsafe { arg.value.data };
                        let value =
                            unsafe { std::slice::from_raw_parts(json_data.data, json_data.len) };
                        RawArgument::Json(name, value.to_vec())
                    }
                    ArgumentType::JsonStream => {
                        let stream_ptr = unsafe { arg.value.stream };
                        let stream_handle = unsafe { &*stream_ptr };
                        let Ok(receiver) = stream_handle.take_receiver() else {
                            return ErrorCode::InvalidArgument;
                        };
                        RawArgument::JsonStream(name, receiver)
                    }
                };
                parsed_args.push(parsed_arg);
            }
            parsed_args
        }
    };

    let func = unsafe { CStr::from_ptr(func) };
    let Ok(func) = func.to_str() else {
        return ErrorCode::InvalidArgument;
    };

    c_try!(sandbox.run(func, args, timeout_in_ms));
    ErrorCode::Ok
}

// ---------------------------------------------------------------------------
// Argument types
// ---------------------------------------------------------------------------

#[repr(C)]
pub enum ArgumentType {
    Json = 0,
    JsonStream = 1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Blob {
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union ArgumentValue {
    pub data: Blob,
    pub stream: *const StreamHandle,
}

#[repr(C)]
pub struct Argument {
    pub r#type: ArgumentType,
    pub name: *const c_char,
    pub value: ArgumentValue,
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

pub struct StreamHandle {
    sender: tokio::sync::mpsc::Sender<Value>,
    receiver: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Value>>>,
}

impl StreamHandle {
    fn take_receiver(&self) -> Result<tokio::sync::mpsc::Receiver<Value>> {
        self.receiver
            .lock()
            .unwrap()
            .take()
            .ok_or(Error::InvalidArgument("Stream receiver already taken"))
    }
}

enum RawArgument {
    Json(Option<String>, Vec<u8>),
    JsonStream(Option<String>, tokio::sync::mpsc::Receiver<Value>),
}

/// Creates a new stream handle for streaming arguments.
///
/// # Safety
///
/// The caller must ensure that `out_stream` is a valid pointer to an
/// uninitialized `Box<StreamHandle>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_stream_create(out_stream: *mut Box<StreamHandle>) -> ErrorCode {
    let (sender, receiver) = tokio::sync::mpsc::channel(1024);
    let stream = Box::new(StreamHandle {
        sender,
        receiver: std::sync::Mutex::new(Some(receiver)),
    });
    unsafe { out_stream.write(stream) };
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
    stream: &StreamHandle,
    data: *const u8,
    len: usize,
    blocking: c_int,
) -> ErrorCode {
    let data = unsafe { std::slice::from_raw_parts(data, len) };
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

    if blocking != 0 {
        // Blocking send - waits until space is available
        if stream.sender.blocking_send(value) == Ok(()) {
            ErrorCode::Ok
        } else {
            let err = Error::StreamClosed;
            crate::error::set_last_error(err);
            ErrorCode::StreamClosed
        }
    } else {
        // Non-blocking send - returns immediately if full
        match stream.sender.try_send(value) {
            Ok(()) => ErrorCode::Ok,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let err = Error::StreamFull;
                crate::error::set_last_error(err);
                ErrorCode::StreamFull
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                let err = Error::StreamClosed;
                crate::error::set_last_error(err);
                ErrorCode::StreamClosed
            }
        }
    }
}

/// Signals the end of a stream.
///
/// After calling this function, no more data can be pushed to the stream.
#[unsafe(no_mangle)]
pub extern "C" fn isola_stream_end(_stream: Box<StreamHandle>) -> ErrorCode {
    // Dropping the sender will close the channel
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn isola_stream_destroy(_stream: Box<StreamHandle>) {}

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
    body: &crate::env::HttpResponseBody,
    status: u16,
    headers: *const crate::env::HttpHeader,
    headers_len: usize,
) -> ErrorCode {
    let owned_headers = if headers_len == 0 || headers.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(headers, headers_len) }
            .iter()
            .map(|h| {
                let name = unsafe { std::slice::from_raw_parts(h.name, h.name_len) }.to_vec();
                let value = unsafe { std::slice::from_raw_parts(h.value, h.value_len) }.to_vec();
                (name, value)
            })
            .collect()
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
    body: &crate::env::HttpResponseBody,
    data: *const u8,
    len: usize,
) -> ErrorCode {
    let chunk = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    if body.send(bytes::Bytes::from(chunk)).is_err() {
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
/// `body` must be a live handle obtained from an `http_request` callback.
/// After this call the pointer is invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn isola_http_response_body_close(body: *mut crate::env::HttpResponseBody) {
    // Dropping the sender signals EOF to the receiver stream.
    drop(unsafe { Box::from_raw(body) });
}
