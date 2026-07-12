use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use futures::future::join_all;
use isola_runtime::{
    block_on,
    wasi_http::{self, HttpRequest, HttpResponse},
};
use pyo3::{PyRefMut, pyclass, pymethods};

use crate::wasm::isola::script::host;

enum PollState {
    Ready,
    Sleep(Instant),
}

#[pyclass]
pub struct PyPollable {
    state: PollState,
}

impl Default for PyPollable {
    fn default() -> Self {
        Self {
            state: PollState::Ready,
        }
    }
}

impl PyPollable {
    pub fn sleep(duration: Duration) -> Self {
        Self {
            state: Instant::now()
                .checked_add(duration)
                .map_or(PollState::Ready, PollState::Sleep),
        }
    }

    pub fn is_ready(&self) -> bool {
        match self.state {
            PollState::Ready => true,
            PollState::Sleep(ready_at) => Instant::now() >= ready_at,
        }
    }

    pub fn wait_until_ready(&self) {
        if let PollState::Sleep(ready_at) = self.state
            && let Some(remaining) = ready_at.checked_duration_since(Instant::now())
        {
            std::thread::sleep(remaining);
        }
    }

    pub const fn ready_at(&self) -> Option<Instant> {
        match self.state {
            PollState::Sleep(ready_at) => Some(ready_at),
            PollState::Ready => None,
        }
    }
}

#[pymethods]
impl PyPollable {
    fn subscribe(mut slf: PyRefMut<'_, Self>) -> Option<PyRefMut<'_, Self>> {
        if slf.is_ready() {
            slf.state = PollState::Ready;
            None
        } else {
            Some(slf)
        }
    }

    const fn get(&self) {
        let _ = self;
    }

    const fn release(&mut self) {
        self.state = PollState::Ready;
    }

    fn wait(&mut self) {
        self.wait_until_ready();
        self.state = PollState::Ready;
    }
}

/// A host call submitted by the guest but not necessarily executed yet.
///
/// `result` stays `None` until the call is driven, either concurrently with
/// other pending calls by [`drive_pending_calls`] (the async path, invoked from
/// the `PollLoop`) or synchronously on its own by [`drive_one`] (the blocking
/// path).
enum DeferredCall {
    Host {
        call_type: String,
        payload: Vec<u8>,
        result: Option<Result<Vec<u8>, String>>,
    },
    Http {
        request: HttpRequest,
        result: Option<Result<HttpResponse, String>>,
    },
}

thread_local! {
    static CALLS: RefCell<Vec<Option<DeferredCall>>> = const { RefCell::new(Vec::new()) };
}

/// Register a deferred host call, returning a handle into the call registry.
#[expect(clippy::cast_possible_truncation)]
pub fn register_call(call_type: String, payload: Vec<u8>) -> u32 {
    CALLS.with(|c| {
        let mut c = c.borrow_mut();
        let op = DeferredCall::Host {
            call_type,
            payload,
            result: None,
        };
        for (i, slot) in c.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(op);
                return i as u32;
            }
        }
        let idx = c.len();
        c.push(Some(op));
        idx as u32
    })
}

pub fn register_http(request: HttpRequest) -> u32 {
    CALLS.with(|c| {
        let mut c = c.borrow_mut();
        let op = DeferredCall::Http {
            request,
            result: None,
        };
        if let Some((i, slot)) = c.iter_mut().enumerate().find(|(_, s)| s.is_none()) {
            *slot = Some(op);
            u32::try_from(i).expect("deferred call index exceeds u32")
        } else {
            let i = c.len();
            c.push(Some(op));
            u32::try_from(i).expect("deferred call index exceeds u32")
        }
    })
}

/// Execute every registered host call that has not been driven yet, **all
/// concurrently** in a single `block_on`. Awaited-together calls (e.g.
/// `asyncio.gather`) are all registered before the `PollLoop` drives them, so
/// their host round-trips overlap. Sequential `await`s register one call at a
/// time and therefore still run serially.
pub fn drive_pending_calls() {
    enum Req {
        Host(String, Vec<u8>),
        Http(HttpRequest),
    }
    enum Res {
        Host(Result<Vec<u8>, String>),
        Http(Result<HttpResponse, String>),
    }
    let reqs: Vec<(usize, Req)> = CALLS.with(|c| {
        c.borrow()
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| match slot {
                Some(DeferredCall::Host {
                    call_type,
                    payload,
                    result: None,
                }) => Some((i, Req::Host(call_type.clone(), payload.clone()))),
                Some(DeferredCall::Http {
                    request,
                    result: None,
                }) => Some((i, Req::Http(request.clone()))),
                _ => None,
            })
            .collect()
    });
    if reqs.is_empty() {
        return;
    }

    let results = block_on(join_all(reqs.into_iter().map(|(i, req)| async move {
        (
            i,
            match req {
                Req::Host(call_type, payload) => {
                    Res::Host(host::hostcall(call_type, payload).await)
                }
                Req::Http(request) => Res::Http(wasi_http::send(request).await),
            },
        )
    })));

    CALLS.with(|c| {
        let mut c = c.borrow_mut();
        for (i, res) in results {
            match (c.get_mut(i), res) {
                (Some(Some(DeferredCall::Host { result, .. })), Res::Host(v)) => *result = Some(v),
                (Some(Some(DeferredCall::Http { result, .. })), Res::Http(v)) => *result = Some(v),
                _ => {}
            }
        }
    });
}

/// Remove a call from the registry and hand back its slot contents, if any.
fn take_call(handle: u32) -> Option<DeferredCall> {
    CALLS.with(|c| {
        c.borrow_mut()
            .get_mut(handle as usize)
            .and_then(Option::take)
    })
}

/// Consume a driven call's result (async path: the `PollLoop` has already run
/// `drive_pending_calls`).
pub fn take_result(handle: u32) -> Result<Vec<u8>, String> {
    match take_call(handle) {
        Some(DeferredCall::Host {
            result: Some(result),
            ..
        }) => result,
        _ => Err("invalid or undriven call handle".to_string()),
    }
}

/// Drive a single call to completion synchronously and consume it (blocking
/// path). If it was already driven, returns the cached result.
pub fn drive_one(handle: u32) -> Result<Vec<u8>, String> {
    match take_call(handle) {
        Some(DeferredCall::Host {
            result: Some(result),
            ..
        }) => result,
        Some(DeferredCall::Host {
            call_type, payload, ..
        }) => block_on(host::hostcall(call_type, payload)),
        _ => Err("invalid call handle".to_string()),
    }
}

pub fn take_http_result(handle: u32) -> Result<HttpResponse, String> {
    match take_call(handle) {
        Some(DeferredCall::Http {
            result: Some(result),
            ..
        }) => result,
        _ => Err("invalid or undriven HTTP handle".to_string()),
    }
}

pub fn drive_one_http(handle: u32) -> Result<HttpResponse, String> {
    match take_call(handle) {
        Some(DeferredCall::Http {
            result: Some(result),
            ..
        }) => result,
        Some(DeferredCall::Http { request, .. }) => block_on(wasi_http::send(request)),
        _ => Err("invalid HTTP handle".to_string()),
    }
}

/// Drop a call without consuming its result.
pub fn release_call(handle: u32) {
    CALLS.with(|c| {
        if let Some(slot) = c.borrow_mut().get_mut(handle as usize) {
            *slot = None;
        }
    });
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
