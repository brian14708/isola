use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use futures::future::join_all;
use isola_runtime::{
    block_on,
    wasi_http::{self, HttpRequest, HttpResponse},
};
use rquickjs::{Array, Ctx, Function, Object, Value};

use super::isola::script::host;
use crate::serde as js_serde;

pub enum PendingOp {
    /// A host call submitted by the guest. `result` stays `None` until the poll
    /// loop drives it in `drive_pending`; multiple un-driven calls are then
    /// issued concurrently.
    HostCall {
        call_type: String,
        payload: Vec<u8>,
        result: Option<Result<Vec<u8>, String>>,
    },
    Http {
        request: HttpRequest,
        result: Option<Result<HttpResponse, String>>,
    },
    Sleep(Option<Instant>),
}

impl PendingOp {
    fn is_ready(&self) -> bool {
        match self {
            Self::HostCall { result, .. } => result.is_some(),
            Self::Http { result, .. } => result.is_some(),
            Self::Sleep(None) => true,
            Self::Sleep(Some(ready_at)) => Instant::now() >= *ready_at,
        }
    }
}

/// Build a deferred plain `hostcall` op (driven later, possibly concurrently).
pub const fn hostcall(call_type: String, payload: Vec<u8>) -> PendingOp {
    PendingOp::HostCall {
        call_type,
        payload,
        result: None,
    }
}

/// Build a deferred HTTP request op. `url` is retained to build the response.
pub const fn http(request: HttpRequest) -> PendingOp {
    PendingOp::Http {
        request,
        result: None,
    }
}

thread_local! {
    static PENDING: RefCell<Vec<Option<PendingOp>>> = const { RefCell::new(Vec::new()) };
}

#[expect(clippy::cast_possible_truncation)]
pub fn register(op: PendingOp) -> u32 {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        for (i, slot) in p.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(op);
                return i as u32;
            }
        }
        let idx = p.len();
        p.push(Some(op));
        idx as u32
    })
}

fn take(handle: u32) -> rquickjs::Result<PendingOp> {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let slot = p
            .get_mut(handle as usize)
            .ok_or_else(|| invalid_handle_error(handle))?;
        slot.take().ok_or_else(|| invalid_handle_error(handle))
    })
}

fn invalid_handle_error(handle: u32) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        "async",
        "handle",
        &format!("invalid or already-consumed handle: {handle}"),
    )
}

#[expect(clippy::cast_possible_truncation)]
pub fn poll_all() -> Vec<u32> {
    PENDING.with(|p| {
        p.borrow()
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().filter(|op| op.is_ready()).map(|_| i as u32))
            .collect()
    })
}

/// Earliest instant at which a currently-pending op becomes ready.
///
/// Only timed sleeps (`Sleep(Some)`) are not ready immediately, so this is the
/// next wakeup deadline for the poll loop. When `poll_all()` returns empty,
/// every pending op is a timed sleep and this is `Some`.
pub fn next_deadline() -> Option<Instant> {
    PENDING.with(|p| {
        p.borrow()
            .iter()
            .filter_map(|slot| match slot {
                Some(PendingOp::Sleep(Some(at))) => Some(*at),
                _ => None,
            })
            .min()
    })
}

/// Drive every host call that has been submitted but not yet executed.
///
/// All un-driven host calls are issued **concurrently** in a single
/// `block_on`, so `Promise.all([fetch(a), fetch(b)])` (which submits both ops
/// before the poll loop runs) overlaps their host round-trips. Sequential
/// `await fetch(a); await fetch(b)` still runs serially because only one op is
/// outstanding when the loop drives.
///
/// Returns `true` if at least one host call was driven (the caller should
/// re-poll for readiness), `false` if there was nothing to drive.
pub fn drive_pending() -> bool {
    enum Req {
        Host(String, Vec<u8>),
        Http(HttpRequest),
    }
    enum Res {
        Host(Result<Vec<u8>, String>),
        Http(Result<HttpResponse, String>),
    }
    let reqs: Vec<(usize, Req)> = PENDING.with(|p| {
        p.borrow()
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| match slot {
                Some(PendingOp::HostCall {
                    call_type,
                    payload,
                    result: None,
                    ..
                }) => Some((i, Req::Host(call_type.clone(), payload.clone()))),
                Some(PendingOp::Http {
                    request,
                    result: None,
                }) => Some((i, Req::Http(request.clone()))),
                _ => None,
            })
            .collect()
    });
    if reqs.is_empty() {
        return false;
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

    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        for (i, res) in results {
            match (p.get_mut(i), res) {
                (Some(Some(PendingOp::HostCall { result, .. })), Res::Host(value)) => {
                    *result = Some(value);
                }
                (Some(Some(PendingOp::Http { result, .. })), Res::Http(value)) => {
                    *result = Some(value);
                }
                _ => {}
            }
        }
    });
    true
}

pub fn resolve_ready(ctx: &Ctx<'_>, ready_handles: &[u32]) -> rquickjs::Result<()> {
    let arr = Array::new(ctx.clone())?;
    for (i, &h) in ready_handles.iter().enumerate() {
        arr.set(i, h)?;
    }

    let globals = ctx.globals();
    let async_obj: Object<'_> = globals.get("_isola_async")?;
    let resolve_fn: Function<'_> = async_obj.get("_resolve")?;
    resolve_fn.call::<_, ()>((arr,))?;
    Ok(())
}

pub fn has_pending() -> bool {
    PENDING.with(|p| p.borrow().iter().any(Option::is_some))
}

pub fn recv_http<'js>(ctx: &Ctx<'js>, handle: u32) -> rquickjs::Result<Object<'js>> {
    let op = take(handle)?;
    match op {
        PendingOp::Http {
            result: Some(response),
            request,
            ..
        } => {
            let response =
                response.map_err(|e| rquickjs::Error::new_from_js_message("fetch", "error", &e))?;
            super::http::build_response_object(ctx, response, request.url().as_str())
                .map_err(|e| rquickjs::Error::new_from_js_message("fetch", "error", &e))
        }
        _ => Err(rquickjs::Error::new_from_js_message(
            "recv",
            "error",
            "handle is not a completed HTTP operation",
        )),
    }
}

pub fn finish_hostcall<'js>(ctx: &Ctx<'js>, handle: u32) -> rquickjs::Result<Value<'js>> {
    let op = take(handle)?;
    match op {
        PendingOp::HostCall {
            result: Some(result),
            ..
        } => {
            let cbor_result = result
                .map_err(|e| rquickjs::Error::new_from_js_message("hostcall", "error", &e))?;
            js_serde::cbor_to_js(ctx, &cbor_result)
                .map_err(|e| rquickjs::Error::new_from_js_message("cbor", "value", &e))
        }
        _ => Err(rquickjs::Error::new_from_js_message(
            "hostcall",
            "error",
            "handle is not a completed hostcall operation",
        )),
    }
}

pub fn finish_sleep(handle: u32) -> rquickjs::Result<()> {
    let op = take(handle)?;
    if matches!(op, PendingOp::Sleep(_)) {
        Ok(())
    } else {
        Err(rquickjs::Error::new_from_js_message(
            "sleep",
            "error",
            "handle is not a sleep operation",
        ))
    }
}

pub fn register_js(ctx: &Ctx<'_>) {
    let globals = ctx.globals();
    let sys: Object<'_> = globals.get("_isola_sys").unwrap();

    sys.set(
        "_drain_jobs",
        rquickjs::Function::new(ctx.clone(), || {}).unwrap(),
    )
    .unwrap();

    sys.set(
        "_finish_sleep",
        rquickjs::Function::new(ctx.clone(), finish_sleep).unwrap(),
    )
    .unwrap();
}

pub fn sleep(duration: f64) -> PendingOp {
    if duration.is_finite() && duration > 0.0 {
        PendingOp::Sleep(Instant::now().checked_add(Duration::from_secs_f64(duration)))
    } else {
        PendingOp::Sleep(None)
    }
}
