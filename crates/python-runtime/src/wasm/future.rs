use std::time::{Duration, Instant};

use isola_runtime::{
    Deadline,
    pending::{self, Output, Take},
    wasi_http::{HttpRequest, HttpResponse},
};
use pyo3::{PyRefMut, pyclass, pymethods};

#[pyclass]
#[derive(Default)]
pub struct PyPollable {
    deadline: Deadline,
}

impl PyPollable {
    pub fn sleep(duration: Duration) -> Self {
        Self {
            deadline: Deadline::after(duration),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.deadline.is_ready()
    }

    pub fn wait_until_ready(&self) {
        self.deadline.wait();
    }

    pub const fn ready_at(&self) -> Option<Instant> {
        self.deadline.ready_at()
    }
}

#[pymethods]
impl PyPollable {
    fn subscribe(mut slf: PyRefMut<'_, Self>) -> Option<PyRefMut<'_, Self>> {
        if slf.is_ready() {
            slf.deadline.clear();
            None
        } else {
            Some(slf)
        }
    }

    const fn get(&self) {
        let _ = self;
    }

    const fn release(&mut self) {
        self.deadline.clear();
    }

    fn wait(&mut self) {
        self.wait_until_ready();
        self.deadline.clear();
    }
}

/// Register a deferred host call, returning a handle into the call registry.
pub fn register_call(call_type: String, payload: Vec<u8>) -> u32 {
    pending::register_hostcall(call_type, payload)
}

pub fn register_http(request: HttpRequest) -> u32 {
    pending::register_http(request)
}

/// Execute every registered host call that has not been driven yet, **all
/// concurrently** in a single `block_on`. Awaited-together calls (e.g.
/// `asyncio.gather`) are all registered before the `PollLoop` drives them, so
/// their host round-trips overlap. Sequential `await`s register one call at a
/// time and therefore still run serially.
pub fn drive_pending_calls() {
    let _ = pending::drive_pending();
}

/// Consume a driven call's result (async path: the `PollLoop` has already run
/// `drive_pending_calls`).
pub fn take_result(handle: u32) -> Result<Vec<u8>, String> {
    match pending::take(handle) {
        Ok(Take::Ready(Output::Host(result))) => result,
        _ => Err("invalid or undriven call handle".to_string()),
    }
}

/// Drive a single call to completion synchronously and consume it (blocking
/// path). If it was already driven, returns the cached result.
pub fn drive_one(handle: u32) -> Result<Vec<u8>, String> {
    match pending::drive_one(handle) {
        Ok(Output::Host(result)) => result,
        _ => Err("invalid call handle".to_string()),
    }
}

pub fn take_http_result(handle: u32) -> Result<HttpResponse, String> {
    match pending::take(handle) {
        Ok(Take::Ready(Output::Http { response, .. })) => response,
        _ => Err("invalid or undriven HTTP handle".to_string()),
    }
}

pub fn drive_one_http(handle: u32) -> Result<HttpResponse, String> {
    match pending::drive_one(handle) {
        Ok(Output::Http { response, .. }) => response,
        _ => Err("invalid HTTP handle".to_string()),
    }
}

/// Drop a call without consuming its result.
pub fn release_call(handle: u32) {
    pending::release(handle);
}

macro_rules! create_future {
    ($name:ident, http -> $type:ty) => {
        #[::pyo3::prelude::pyclass]
        struct $name {
            handle: u32,
        }
        impl $name {
            const fn new(handle: u32) -> Self {
                Self { handle }
            }
        }
        #[::pyo3::prelude::pymethods]
        impl $name {
            fn wait(slf: ::pyo3::PyRef<'_, Self>) -> PyResult<$type> {
                crate::wasm::future::drive_one_http(slf.handle).try_into()
            }
            fn subscribe(_slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                Some(crate::wasm::future::PyPollable::default())
            }
            fn get(slf: ::pyo3::PyRef<'_, Self>) -> PyResult<$type> {
                crate::wasm::future::take_http_result(slf.handle).try_into()
            }
            fn release(slf: ::pyo3::PyRef<'_, Self>) {
                crate::wasm::future::release_call(slf.handle);
            }
        }
    };
    ($name:ident, $type:ty) => {
        #[::pyo3::prelude::pyclass]
        struct $name {
            handle: u32,
        }

        impl $name {
            const fn new(handle: u32) -> Self {
                Self { handle }
            }
        }

        #[::pyo3::prelude::pymethods]
        impl $name {
            fn wait(slf: ::pyo3::PyRef<'_, Self>) -> PyResult<$type> {
                crate::wasm::future::drive_one(slf.handle).try_into()
            }

            fn subscribe(_slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                Some(crate::wasm::future::PyPollable::default())
            }

            fn get(slf: ::pyo3::PyRef<'_, Self>) -> PyResult<$type> {
                crate::wasm::future::take_result(slf.handle).try_into()
            }

            fn release(slf: ::pyo3::PyRef<'_, Self>) {
                crate::wasm::future::release_call(slf.handle);
            }
        }
    };
    ($name:ident, $convert:ident -> $type:ty) => {
        #[::pyo3::prelude::pyclass]
        struct $name {
            handle: u32,
        }

        impl $name {
            const fn new(handle: u32) -> Self {
                Self { handle }
            }
        }

        #[::pyo3::prelude::pymethods]
        impl $name {
            fn wait(slf: ::pyo3::PyRef<'_, Self>) -> $type {
                let py = slf.py();
                $convert(py, crate::wasm::future::drive_one(slf.handle))
            }

            fn subscribe(_slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                Some(crate::wasm::future::PyPollable::default())
            }

            fn get(slf: ::pyo3::PyRef<'_, Self>) -> $type {
                let py = slf.py();
                $convert(py, crate::wasm::future::take_result(slf.handle))
            }

            fn release(slf: ::pyo3::PyRef<'_, Self>) {
                crate::wasm::future::release_call(slf.handle);
            }
        }
    };
}

pub(crate) use create_future;
