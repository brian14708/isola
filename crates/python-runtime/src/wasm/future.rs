use isola_runtime::{
    Deadline,
    pending::{self, Output, Take},
    wasi_http::{HttpRequest, HttpResponse},
};
use pyo3::{PyResult, pyclass, pymethods};

#[pyclass]
#[derive(Default)]
pub struct PyPollable {
    state: PollableState,
}

#[derive(Clone, Copy, Default)]
enum PollableState {
    #[default]
    Ready,
    Operation(u32),
}

impl PyPollable {
    pub fn sleep(deadline: Deadline) -> Self {
        if deadline.is_ready() {
            Self::default()
        } else {
            Self::operation(pending::register_sleep(deadline))
        }
    }

    pub(crate) const fn operation(handle: u32) -> Self {
        Self {
            state: PollableState::Operation(handle),
        }
    }

    pub fn is_ready(&self) -> bool {
        match self.state {
            PollableState::Ready => true,
            PollableState::Operation(handle) => pending::is_ready(handle),
        }
    }
}

#[pymethods]
impl PyPollable {
    fn subscribe(&self) -> Option<Self> {
        if self.is_ready() {
            None
        } else {
            Some(Self { state: self.state })
        }
    }

    fn get(&self) -> PyResult<()> {
        match self.state {
            PollableState::Ready => Ok(()),
            PollableState::Operation(handle) => match pending::take(handle) {
                Ok(Take::Ready(Output::Sleep)) => Ok(()),
                Ok(Take::Ready(Output::Host(_) | Output::Http { .. })) => {
                    Err(pyo3::exceptions::PyRuntimeError::new_err(
                        "operation result must be read from its owner",
                    ))
                }
                Ok(Take::Pending) => Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "operation is not ready",
                )),
                Err(error) => Err(pyo3::exceptions::PyRuntimeError::new_err(error.to_string())),
            },
        }
    }

    fn release(&self) {
        if let PollableState::Operation(handle) = self.state {
            pending::release(handle);
        }
    }

    fn wait(&self) -> PyResult<()> {
        match self.state {
            PollableState::Ready => Ok(()),
            PollableState::Operation(handle) => match pending::drive_one(handle) {
                Ok(Output::Sleep) => Ok(()),
                Ok(Output::Host(_) | Output::Http { .. }) => {
                    Err(pyo3::exceptions::PyRuntimeError::new_err(
                        "operation result must be read from its owner",
                    ))
                }
                Err(error) => Err(pyo3::exceptions::PyRuntimeError::new_err(error.to_string())),
            },
        }
    }
}

/// Register a deferred host call, returning a handle into the call registry.
pub fn register_call(call_type: String, payload: Vec<u8>) -> u32 {
    pending::register_hostcall(call_type, payload)
}

pub fn register_http(request: HttpRequest) -> u32 {
    pending::register_http(request)
}

/// Start deferred operations and wait until the first operation is ready.
pub fn drive_pending_calls(step: impl FnMut() -> pending::Drive) -> bool {
    pending::drive_pending(step)
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
            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                let pollable = crate::wasm::future::PyPollable::operation(slf.handle);
                (!pollable.is_ready()).then_some(pollable)
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

            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                let pollable = crate::wasm::future::PyPollable::operation(slf.handle);
                (!pollable.is_ready()).then_some(pollable)
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

            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                let pollable = crate::wasm::future::PyPollable::operation(slf.handle);
                (!pollable.is_ready()).then_some(pollable)
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
