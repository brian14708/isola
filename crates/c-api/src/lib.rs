#![warn(clippy::pedantic)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_safety_doc)]
#![allow(dead_code)]

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    future::Future,
    path::PathBuf,
    pin::Pin,
    time::Duration,
};

use promptkit_executor::{
    VmManager,
    vm::{OutputCallback, Vm, VmRun},
};
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
    vmm: Option<VmManager<Env>>,
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
                    #[allow(clippy::cast_sign_loss)]
                    {
                        n as usize
                    },
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
        Ok(Box::new(Self { rt, vmm: None }))
    }

    fn set_config(&self, _key: &CStr, _value: &CStr) -> Result<()> {
        _ = self;
        todo!();
    }

    fn load(&mut self, mut path: PathBuf) -> Result<()> {
        if self.vmm.is_some() {
            return Err(Error::InvalidArgument("Vm manager already loaded"));
        }
        path.push("promptkit_python.wasm");
        self.rt.block_on(async {
            self.vmm = Some(
                VmManager::new(&path)
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to load VM manager: {e}")))?,
            );
            Ok(())
        })
    }

    fn new_vm(&self) -> Result<VmHandle<'_>> {
        let Some(vmm) = &self.vmm else {
            return Err(Error::InvalidArgument("Vm manager not loaded"));
        };
        let vm = self
            .rt
            .block_on(vmm.create([0; 32], Env::shared().clone()))
            .map_err(|e| Error::Internal(format!("Failed to create VM: {e}")))?;
        Ok(VmHandle {
            ctx: self,
            inner: VmInner::Pending { vm, callback: None },
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_create(
    nr_thread: c_int,
    out_context: *mut Box<ContextHandle>,
) -> ErrorCode {
    let ctx = c_try!(ContextHandle::new(nr_thread));
    unsafe { out_context.write(ctx) };
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_initialize(
    ctx: &mut ContextHandle,
    path: *const c_char,
) -> ErrorCode {
    let path = unsafe { CStr::from_ptr(path) };
    let path = c_try!(match path.to_str() {
        Ok(p) => Ok(p),
        Err(_) => Err(Error::InvalidArgument("Invalid path string")),
    });
    c_try!(ctx.load(path.into()));
    ErrorCode::Ok
}

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
struct Callback {
    callback: extern "C" fn(CallbackEvent, *const u8, usize, *mut c_void),
    user_data: *mut c_void,
}

unsafe impl Send for Callback {}
unsafe impl Sync for Callback {}

impl OutputCallback for Callback {
    fn on_result(
        &mut self,
        item: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), anyhow::Error>> + Send>> {
        let data = promptkit_cbor::cbor_to_json(&item).unwrap();
        (self.callback)(
            CallbackEvent::ResultJson,
            data.as_ptr(),
            data.len(),
            self.user_data,
        );
        Box::pin(async { Ok(()) })
    }
}

enum VmInner {
    Uninitialized,
    Pending {
        vm: Vm<Env>,
        callback: Option<Callback>,
    },
    Running {
        run: VmRun<Env>,
        callback: Callback,
    },
}

pub struct VmHandle<'a> {
    ctx: &'a ContextHandle,
    inner: VmInner,
}

impl VmHandle<'_> {
    fn set_config(&mut self, _key: &CStr, _value: &CStr) -> Result<()> {
        todo!()
    }

    fn set_callback(&mut self, callback: Callback) -> Result<()> {
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
            VmInner::Pending { vm, callback } => {
                let output_callback = callback.ok_or(Error::InvalidArgument("Callback not set"))?;
                let mut run = vm.run(output_callback.clone());
                self.ctx
                    .rt
                    .block_on(run.exec(|guest, store| guest.call_initialize(store, true)))
                    .map_err(|e| Error::Internal(format!("VM initialization failed: {e}")))?;
                self.inner = VmInner::Running {
                    run,
                    callback: output_callback,
                };
                Ok(())
            }
            _ => Err(Error::InvalidArgument("Vm not loaded")),
        }
    }

    fn load_script(&mut self, input: &str, timeout_in_ms: u64) -> Result<()> {
        match &mut self.inner {
            VmInner::Running { run, .. } => {
                self.ctx
                    .rt
                    .block_on(run.exec(|guest, store| async move {
                        tokio::time::timeout(
                            Duration::from_millis(timeout_in_ms),
                            guest.call_eval_script(store, input),
                        )
                        .await
                    }))
                    .map_err(|_| Error::Internal("Script execution timeout".to_string()))??
                    .map_err(|e| Error::Internal(format!("Script loading failed: {e}")))?;

                Ok(())
            }
            _ => Err(Error::InvalidArgument("Vm not running")),
        }
    }

    fn run(&mut self, func: &str, args: Vec<RawArgument>, timeout_in_ms: u64) -> Result<()> {
        match &mut self.inner {
            VmInner::Running { run, callback } => {
                let args = args
                    .into_iter()
                    .map(|arg| match arg {
                        RawArgument::Json(name, value) => {
                            // Avoid temporary String allocation
                            let json_str = std::str::from_utf8(&value).map_err(|_| {
                                Error::InvalidArgument("Invalid UTF-8 in JSON argument")
                            })?;
                            let cbor_data = promptkit_cbor::json_to_cbor(json_str)
                                .map_err(|_| Error::InvalidArgument("Invalid JSON format"))?;
                            Ok(promptkit_executor::vm::exports::Argument {
                                name,
                                value: promptkit_executor::vm::exports::Value::Cbor(cbor_data),
                            })
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                let v = self
                    .ctx
                    .rt
                    .block_on(run.exec(|guest, store| async move {
                        tokio::time::timeout(
                            Duration::from_millis(timeout_in_ms),
                            guest.call_call_func(store, func, &args),
                        )
                        .await
                    }))
                    .map_err(|_| Error::Internal("Operation timeout".to_string()))??
                    .map_err(|e| Error::Internal(format!("VM execution failed: {e}")))?;

                match v {
                    Some(data) => {
                        let json_data = promptkit_cbor::cbor_to_json(&data).map_err(|_| {
                            Error::Internal("CBOR to JSON conversion failed".to_string())
                        })?;
                        (callback.callback)(
                            CallbackEvent::EndJson,
                            json_data.as_ptr(),
                            json_data.len(),
                            callback.user_data,
                        );
                    }
                    None => {
                        (callback.callback)(
                            CallbackEvent::EndJson,
                            std::ptr::null(),
                            0,
                            callback.user_data,
                        );
                    }
                }

                Ok(())
            }
            _ => Err(Error::InvalidArgument("Vm not running")),
        }
    }
}

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
}

#[repr(C)]
pub struct Argument {
    r#type: ArgumentType,
    name: *const c_char,
    value: *const u8,
    len: usize,
}

enum RawArgument {
    Json(Option<String>, Vec<u8>),
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_vm_load_script(
    vm: &mut VmHandle<'_>,
    input: *const c_char,
    timeout_in_ms: u64,
) -> ErrorCode {
    let input = unsafe { CStr::from_ptr(input) };
    let input = c_try!(match input.to_str() {
        Ok(p) => Ok(p),
        Err(_) => Err(Error::InvalidArgument("Invalid input string")),
    });
    c_try!(vm.load_script(input, timeout_in_ms));
    ErrorCode::Ok
}

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
        unsafe { std::slice::from_raw_parts(args, args_len) }
            .iter()
            .map(|arg| {
                let name = if arg.name.is_null() {
                    None
                } else {
                    let name = unsafe { CStr::from_ptr(arg.name) };
                    let name = name.to_str().expect("Invalid name string");
                    Some(name.to_string())
                };

                let value = unsafe { std::slice::from_raw_parts(arg.value, arg.len) };
                let value = value.to_vec();
                match arg.r#type {
                    ArgumentType::Json => RawArgument::Json(name, value),
                }
            })
            .collect::<Vec<_>>()
    };

    let func = unsafe { CStr::from_ptr(func) };
    let func = func.to_str().expect("Invalid func string");

    c_try!(vm.run(func, args, timeout_in_ms));
    ErrorCode::Ok
}
