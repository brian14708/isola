use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use futures::future::join_all;
use pyo3::{PyRefMut, pyclass, pymethods};
use wit_bindgen::block_on;

use crate::wasm::isola::script::host;

enum PollState {
    Ready,
    Sleep(Instant),
    Released,
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
            PollState::Ready | PollState::Released => true,
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
            PollState::Ready | PollState::Released => None,
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
        self.state = PollState::Released;
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
struct DeferredCall {
    call_type: String,
    payload: Vec<u8>,
    result: Option<Result<Vec<u8>, String>>,
}

thread_local! {
    static CALLS: RefCell<Vec<Option<DeferredCall>>> = const { RefCell::new(Vec::new()) };
}

/// Register a deferred host call, returning a handle into the call registry.
#[expect(clippy::cast_possible_truncation)]
pub fn register_call(call_type: String, payload: Vec<u8>) -> u32 {
    CALLS.with(|c| {
        let mut c = c.borrow_mut();
        let op = DeferredCall {
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

/// Execute every registered host call that has not been driven yet, **all
/// concurrently** in a single `block_on`. Awaited-together calls (e.g.
/// `asyncio.gather`) are all registered before the `PollLoop` drives them, so
/// their host round-trips overlap. Sequential `await`s register one call at a
/// time and therefore still run serially.
pub fn drive_pending_calls() {
    let reqs: Vec<(usize, String, Vec<u8>)> = CALLS.with(|c| {
        c.borrow()
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| match slot {
                Some(DeferredCall {
                    call_type,
                    payload,
                    result: None,
                }) => Some((i, call_type.clone(), payload.clone())),
                _ => None,
            })
            .collect()
    });
    if reqs.is_empty() {
        return;
    }

    let results = block_on(join_all(reqs.into_iter().map(
        |(i, call_type, payload)| async move { (i, host::hostcall(call_type, payload).await) },
    )));

    CALLS.with(|c| {
        let mut c = c.borrow_mut();
        for (i, res) in results {
            if let Some(Some(call)) = c.get_mut(i) {
                call.result = Some(res);
            }
        }
    });
}

/// Consume a driven call's result (async path: the `PollLoop` has already run
/// `drive_pending_calls`).
pub fn take_result(handle: u32) -> Result<Vec<u8>, String> {
    let call = CALLS.with(|c| {
        c.borrow_mut()
            .get_mut(handle as usize)
            .and_then(Option::take)
    });
    match call {
        Some(DeferredCall {
            result: Some(result),
            ..
        }) => result,
        _ => Err("invalid or undriven call handle".to_string()),
    }
}

/// Drive a single call to completion synchronously and consume it (blocking
/// path). If it was already driven, returns the cached result.
pub fn drive_one(handle: u32) -> Result<Vec<u8>, String> {
    let call = CALLS.with(|c| {
        c.borrow_mut()
            .get_mut(handle as usize)
            .and_then(Option::take)
    });
    match call {
        Some(DeferredCall {
            result: Some(result),
            ..
        }) => result,
        Some(DeferredCall {
            call_type, payload, ..
        }) => block_on(host::hostcall(call_type, payload)),
        None => Err("invalid call handle".to_string()),
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
    ($name:ident, $result_type:ty, $type:ty) => {
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
    ($name:ident, $result_type:ty, $convert:ident -> $type:ty) => {
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
