#![warn(clippy::pedantic)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::{CStr, c_char};

use tokio::runtime::{Builder, Runtime};

use crate::error::{Error, ErrorCode, Result};

mod error;

pub struct ContextHandle {
    _rt: Runtime,
}

impl ContextHandle {
    fn new(nr_thread: usize) -> Result<Box<Self>> {
        let rt = if nr_thread == 0 {
            Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?
        } else {
            Builder::new_multi_thread()
                .worker_threads(nr_thread as _)
                .thread_name("promptkit-runner")
                .enable_all()
                .build()
                .map_err(|e| Error::Internal(format!("failed to build runtime: {e}")))?
        };
        Ok(Box::new(Self { _rt: rt }))
    }

    pub fn set_config(&self, _key: &CStr, _value: &CStr) {}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_create(
    nr_thread: usize,
    out_context: *mut Box<ContextHandle>,
) -> ErrorCode {
    let ctx = c_try!(ContextHandle::new(nr_thread));
    unsafe { out_context.write(ctx) };
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_initialize(
    ctx: &ContextHandle,
    path: *const c_char,
) -> ErrorCode {
    _ = ctx;
    if !path.is_null() {
        let _ = unsafe { CStr::from_ptr(path) };
    }
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn promptkit_context_config_set(
    ctx: &ContextHandle,
    key: *const c_char,
    value: *const c_char,
) -> ErrorCode {
    let key = unsafe { CStr::from_ptr(key) };
    let value = unsafe { CStr::from_ptr(value) };
    ctx.set_config(key, value);
    ErrorCode::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn promptkit_context_destroy(_ctx: Box<ContextHandle>) {}
