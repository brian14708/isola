use std::{
    ffi::{CStr, c_char, c_int, c_void},
    path::PathBuf,
    time::Duration,
};

use bytes::Bytes;
use promptkit::{Instance, Runtime as PromptRuntime};
use tokio::runtime::{Builder, Runtime};

use crate::env::Env;
use crate::error::{Error, ErrorCode, Result};

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

pub struct ContextHandle {
    rt: Runtime,
    runtime: Option<PromptRuntime<Env>>,
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
                .thread_name("promptkit-runner")
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?,

            _ => Builder::new_multi_thread()
                .thread_name("promptkit-runner")
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?,
        };
        Ok(Box::new(Self { rt, runtime: None }))
    }

    fn set_config(&self, _key: &CStr, _value: &CStr) -> Result<()> {
        _ = self;
        todo!();
    }

    fn load(&mut self, mut path: PathBuf) -> Result<()> {
        if self.runtime.is_some() {
            return Err(Error::InvalidArgument("Runtime already loaded"));
        }
        path.push("promptkit_python.wasm");

        let parent = path
            .parent()
            .ok_or_else(|| Error::Internal("Wasm path has no parent directory".to_string()))?;

        self.rt.block_on(async {
            let runtime = PromptRuntime::<Env>::builder()
                .cache_path(parent.join("cache"))
                .library_path({
                    let mut lib_dir = std::env::var("WASI_PYTHON_RUNTIME").map_or_else(
                        |_| {
                            let mut lib_dir = parent.to_owned();
                            lib_dir.push("wasm32-wasip2");
                            lib_dir.push("wasi-deps");
                            lib_dir.push("usr");
                            lib_dir.push("local");
                            lib_dir
                        },
                        PathBuf::from,
                    );
                    lib_dir.push("lib");
                    lib_dir
                })
                .build(&path)
                .await
                .map_err(|e| Error::Internal(format!("Failed to load runtime: {e}")))?;
            self.runtime = Some(runtime);
            Ok(())
        })
    }

    fn new_vm(&self) -> Result<VmHandle<'_>> {
        let Some(runtime) = &self.runtime else {
            return Err(Error::InvalidArgument("Runtime not loaded"));
        };
        let instance = self
            .rt
            .block_on(async { runtime.instantiate(None, Env::shared().await).await })
            .map_err(|e| Error::Internal(format!("Failed to create instance: {e}")))?;
        Ok(VmHandle {
            ctx: self,
            inner: VmInner::Pending {
                instance,
                callback: None,
            },
        })
    }
}

/// Creates a new promptkit context with the specified number of threads.
///
/// # Safety
///
/// The caller must ensure that `out_context` is a valid pointer to an
/// uninitialized `Box<ContextHandle>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_create(
    nr_thread: c_int,
    out_context: *mut Box<ContextHandle>,
) -> ErrorCode {
    let ctx = c_try!(ContextHandle::new(nr_thread));
    unsafe { out_context.write(ctx) };
    ErrorCode::Ok
}

/// Initializes the promptkit context with the specified path.
///
/// # Safety
///
/// The caller must ensure that `path` is a valid, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_initialize(
    ctx: &mut ContextHandle,
    path: *const c_char,
) -> ErrorCode {
    let path = unsafe { CStr::from_ptr(path) };
    let path = c_try!(
        path.to_str()
            .map_or_else(|_| Err(Error::InvalidArgument("Invalid path string")), Ok)
    );
    c_try!(ctx.load(path.into()));
    ErrorCode::Ok
}

/// Sets a configuration value for the promptkit context.
///
/// # Safety
///
/// The caller must ensure that both `key` and `value` are valid, null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_config_set(
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
pub extern "C" fn promptkit_context_destroy(_ctx: Box<ContextHandle>) {}

#[derive(Clone)]
pub struct Callback {
    callback: extern "C" fn(CallbackEvent, *const u8, usize, *mut c_void),
    user_data: *mut c_void,
}

unsafe impl Send for Callback {}
unsafe impl Sync for Callback {}

impl promptkit::environment::OutputCallback for Callback {
    type Error = std::io::Error;

    async fn on_result(&mut self, item: Bytes) -> std::result::Result<(), Self::Error> {
        let data = promptkit_cbor::cbor_to_json(&item)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        (self.callback)(
            CallbackEvent::ResultJson,
            data.as_ptr(),
            data.len(),
            self.user_data,
        );
        Ok(())
    }

    async fn on_end(&mut self, item: Bytes) -> std::result::Result<(), Self::Error> {
        if item.is_empty() {
            (self.callback)(CallbackEvent::EndJson, std::ptr::null(), 0, self.user_data);
        } else {
            let data = promptkit_cbor::cbor_to_json(&item)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            (self.callback)(
                CallbackEvent::EndJson,
                data.as_ptr(),
                data.len(),
                self.user_data,
            );
        }
        Ok(())
    }
}

enum VmInner {
    Uninitialized,
    Pending {
        instance: Instance<Env>,
        callback: Option<Callback>,
    },
    Running {
        instance: Instance<Env>,
        callback: Callback,
    },
}

pub struct VmHandle<'a> {
    ctx: &'a ContextHandle,
    inner: VmInner,
}

impl VmHandle<'_> {
    fn set_config(&self, _key: &CStr, _value: &CStr) -> Result<()> {
        todo!()
    }

    const fn set_callback(&mut self, callback: Callback) -> Result<()> {
        match &mut self.inner {
            VmInner::Pending { callback: cb, .. } => {
                *cb = Some(callback);
                Ok(())
            }
            _ => Err(Error::InvalidArgument("Callback already set")),
        }
    }

    fn start(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.inner, VmInner::Uninitialized) {
            VmInner::Pending { instance, callback } => {
                let callback = callback.ok_or(Error::InvalidArgument("Callback not set"))?;
                // Instance is already initialized when created
                self.inner = VmInner::Running { instance, callback };
                Ok(())
            }
            _ => Err(Error::InvalidArgument("Instance not in pending state")),
        }
    }

    fn load_script(&mut self, input: &str, timeout_in_ms: u64) -> Result<()> {
        match &mut self.inner {
            VmInner::Running { instance, .. } => {
                self.ctx
                    .rt
                    .block_on(async {
                        tokio::time::timeout(
                            Duration::from_millis(timeout_in_ms),
                            instance.eval_script(input),
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
        match std::mem::replace(&mut self.inner, VmInner::Uninitialized) {
            VmInner::Running {
                mut instance,
                callback,
            } => {
                // Convert arguments to promptkit format
                let promptkit_args: Vec<_> = args
                    .into_iter()
                    .map(|arg| -> Result<(Option<String>, promptkit::Argument)> {
                        match arg {
                            RawArgument::Json(name, value) => {
                                let json_str = std::str::from_utf8(&value).map_err(|_| {
                                    Error::InvalidArgument("Invalid UTF-8 in JSON argument")
                                })?;
                                let cbor = promptkit_cbor::json_to_cbor(json_str)
                                    .map_err(|_| Error::InvalidArgument("Invalid JSON format"))?;
                                Ok((name, promptkit::Argument::Cbor(cbor)))
                            }
                            RawArgument::JsonStream(name, receiver) => {
                                let stream =
                                    Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver));
                                Ok((name, promptkit::Argument::CborStream(stream)))
                            }
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;

                let func = func.to_string();
                let result = self.ctx.rt.block_on(async {
                    tokio::time::timeout(
                        Duration::from_millis(timeout_in_ms),
                        instance.execute(&func, promptkit_args, callback.clone()),
                    )
                    .await
                });

                // Restore the instance state
                self.inner = VmInner::Running { instance, callback };

                result
                    .map_err(|_| Error::Internal("Operation timeout".to_string()))?
                    .map_err(|e| Error::Internal(format!("VM execution failed: {e}")))?;

                Ok(())
            }
            _ => Err(Error::InvalidArgument("Instance not running")),
        }
    }
}

/// Creates a new VM instance from the context.
///
/// # Safety
///
/// The caller must ensure that `out_vm` is a valid pointer to an
/// uninitialized `Box<VmHandle>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_vm_create<'a>(
    ctx: &'a mut ContextHandle,
    out_vm: *mut Box<VmHandle<'a>>,
) -> ErrorCode {
    let vm = c_try!(ctx.new_vm());
    unsafe { out_vm.write(Box::new(vm)) };
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn promptkit_vm_destroy(_vm: Box<VmHandle<'_>>) {}

/// Sets a configuration value for the VM.
///
/// # Safety
///
/// The caller must ensure that both `key` and `value` are valid, null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_vm_set_config(
    vm: &mut VmHandle<'_>,
    key: *const c_char,
    value: *const c_char,
) -> ErrorCode {
    let key = unsafe { CStr::from_ptr(key) };
    let value = unsafe { CStr::from_ptr(value) };
    c_try!(vm.set_config(key, value));
    ErrorCode::Ok
}

#[repr(C)]
pub enum CallbackEvent {
    ResultJson = 0,
    EndJson = 4,
    Stdout = 1,
    Stderr = 2,
    Error = 3,
}

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

pub struct StreamHandle {
    sender: tokio::sync::mpsc::Sender<Bytes>,
    receiver: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Bytes>>>,
}

impl StreamHandle {
    fn take_receiver(&self) -> Result<tokio::sync::mpsc::Receiver<Bytes>> {
        self.receiver
            .lock()
            .unwrap()
            .take()
            .ok_or(Error::InvalidArgument("Stream receiver already taken"))
    }
}

enum RawArgument {
    Json(Option<String>, Vec<u8>),
    JsonStream(Option<String>, tokio::sync::mpsc::Receiver<Bytes>),
}

#[unsafe(no_mangle)]
pub extern "C" fn promptkit_vm_set_callback(
    vm: &mut VmHandle<'_>,
    callback: extern "C" fn(CallbackEvent, *const u8, usize, *mut c_void),
    user_data: *mut c_void,
) -> ErrorCode {
    let callback = Callback {
        callback,
        user_data,
    };
    c_try!(vm.set_callback(callback));
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn promptkit_vm_start(vm: &mut VmHandle<'_>) -> ErrorCode {
    c_try!(vm.start());
    ErrorCode::Ok
}

/// Loads a script into the VM.
///
/// # Safety
///
/// The caller must ensure that `input` is a valid, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_vm_load_script(
    vm: &mut VmHandle<'_>,
    input: *const c_char,
    timeout_in_ms: u64,
) -> ErrorCode {
    let input = unsafe { CStr::from_ptr(input) };
    let input = c_try!(
        input
            .to_str()
            .map_or_else(|_| Err(Error::InvalidArgument("Invalid input string")), Ok)
    );
    c_try!(vm.load_script(input, timeout_in_ms));
    ErrorCode::Ok
}

/// Runs a function in the VM with the specified arguments.
///
/// # Safety
///
/// The caller must ensure that:
/// - `func` is a valid, null-terminated C string
/// - `args` is a valid pointer to an array of `Argument` structs of length `args_len`
/// - Each `Argument` in the array has valid pointers and data
///
/// # Panics
///
/// This function may panic if argument names contain invalid UTF-8 sequences.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_vm_run(
    vm: &mut VmHandle<'_>,
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

    c_try!(vm.run(func, args, timeout_in_ms));
    ErrorCode::Ok
}

/// Creates a new stream handle for streaming arguments.
///
/// # Safety
///
/// The caller must ensure that `out_stream` is a valid pointer to an
/// uninitialized `Box<StreamHandle>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_stream_create(out_stream: *mut Box<StreamHandle>) -> ErrorCode {
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
pub unsafe extern "C" fn promptkit_stream_push(
    stream: &StreamHandle,
    data: *const u8,
    len: usize,
    blocking: c_int,
) -> ErrorCode {
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    let bytes = Bytes::copy_from_slice(data);

    if blocking != 0 {
        // Blocking send - waits until space is available
        if stream.sender.blocking_send(bytes) == Ok(()) {
            ErrorCode::Ok
        } else {
            let err = Error::StreamClosed;
            crate::error::set_last_error(err);
            ErrorCode::StreamClosed
        }
    } else {
        // Non-blocking send - returns immediately if full
        match stream.sender.try_send(bytes) {
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
pub extern "C" fn promptkit_stream_end(_stream: Box<StreamHandle>) -> ErrorCode {
    // Dropping the sender will close the channel
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn promptkit_stream_destroy(_stream: Box<StreamHandle>) {}
