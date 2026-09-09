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

    pub(crate) fn wait_blocking(&self) -> PyResult<()> {
        match self.state {
            PollableState::Ready => Ok(()),
            PollableState::Operation(handle) => {
                let _ = pending::drive_pending(|| {
                    if pending::is_ready(handle) {
                        pending::Drive::Suspend
                    } else {
                        pending::Drive::Wait
                    }
                });
                if pending::is_ready(handle) {
                    Ok(())
                } else {
                    Err(pyo3::exceptions::PyRuntimeError::new_err(
                        "operation did not become ready",
                    ))
                }
            }
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
            PollableState::Operation(handle) if pending::is_http_stream_read(handle) => {
                if pending::is_ready(handle) {
                    Ok(())
                } else {
                    Err(pyo3::exceptions::PyRuntimeError::new_err(
                        "operation is not ready",
                    ))
                }
            }
            PollableState::Operation(handle) => match pending::take(handle) {
                Ok(Take::Ready(Output::Sleep)) => Ok(()),
                Ok(Take::Ready(
                    Output::Host(_)
                    | Output::Http { .. }
                    | Output::HttpStream(_)
                    | Output::HttpUploadWrite(_),
                )) => Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "operation result must be read from its owner",
                )),
                Ok(Take::Pending) => Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "operation is not ready",
                )),
                Err(error) => Err(pyo3::exceptions::PyRuntimeError::new_err(error.to_string())),
            },
        }
    }

    fn release(&self) {
        if let PollableState::Operation(handle) = self.state {
            // A response owner consumes stream-read results itself after the
            // pollable wakes. Keep a ready read alive for that owner; pending
            // reads can still be cancelled normally.
            if !pending::is_http_stream_read(handle) || !pending::is_ready(handle) {
                pending::release(handle);
            }
        }
    }

    fn wait(&self) -> PyResult<()> {
        self.wait_blocking()
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

/// Pump the executor until `handle` is ready (or the executor stalls).
fn drive_until_ready(handle: u32) {
    let _ = pending::drive_pending(|| {
        if pending::is_ready(handle) {
            pending::Drive::Suspend
        } else {
            pending::Drive::Wait
        }
    });
}

pub fn drive_one_http(handle: u32) -> Result<HttpResponse, String> {
    drive_until_ready(handle);
    match pending::take(handle) {
        Ok(Take::Ready(Output::Http { response, .. })) => response,
        _ => Err("invalid or undriven HTTP handle".to_string()),
    }
}

pub fn take_http_upload_write_result(handle: u32) -> Result<(), String> {
    match pending::take(handle) {
        Ok(Take::Ready(Output::HttpUploadWrite(result))) => result,
        _ => Err("invalid or undriven HTTP upload handle".to_string()),
    }
}

pub fn drive_one_http_upload_write(handle: u32) -> Result<(), String> {
    drive_until_ready(handle);
    if pending::is_ready(handle) {
        Ok(())
    } else {
        Err("invalid or undriven HTTP upload handle".to_string())
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
