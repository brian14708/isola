use std::cell::RefCell;

use rquickjs::{Array, Ctx, Function, Object, Value};

use super::wasi::{
    self, http::outgoing_handler::FutureIncomingResponse, io::poll::poll as wasi_poll,
};
use crate::serde as js_serde;

pub enum PendingOp {
    Http {
        response: FutureIncomingResponse,
        url: String,
    },
    Hostcall(super::isola::script::host::FutureHostcall),
    Sleep,
}

struct PendingEntry {
    pollable: wasi::io::poll::Pollable,
    op: PendingOp,
}

thread_local! {
    static PENDING: RefCell<Vec<Option<PendingEntry>>> = const { RefCell::new(Vec::new()) };
}

#[allow(clippy::cast_possible_truncation)]
pub fn register(pollable: wasi::io::poll::Pollable, op: PendingOp) -> u32 {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = PendingEntry { pollable, op };
        // Find first free slot
        for (i, slot) in p.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(entry);
                return i as u32;
            }
        }
        let idx = p.len();
        p.push(Some(entry));
        idx as u32
    })
}

/// Remove a pending entry and return only the operation.
/// The pollable is dropped first to satisfy WASI resource parenting
/// (the pollable is a child of the operation resource).
fn take(handle: u32) -> rquickjs::Result<PendingOp> {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let slot = p
            .get_mut(handle as usize)
            .ok_or_else(|| invalid_handle_error(handle))?;
        let entry = slot.take().ok_or_else(|| invalid_handle_error(handle))?;
        // Drop pollable before returning the op — the pollable is a child
        // resource of the op (FutureIncomingResponse / FutureHostcall) in
        // the WASI resource table, so it must be dropped first.
        drop(entry.pollable);
        Ok(entry.op)
    })
}

fn invalid_handle_error(handle: u32) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        "async",
        "handle",
        &format!("invalid or already-consumed handle: {handle}"),
    )
}

/// Poll all registered pollables via `wasi:io/poll::poll`.
/// Returns handles of ready entries (entries remain in the registry until
/// `take()`).
#[allow(clippy::cast_possible_truncation)]
pub fn poll_all() -> Vec<u32> {
    PENDING.with(|p| {
        let p = p.borrow();

        // Collect active entries: (slot_index, &pollable)
        let active: Vec<(usize, &wasi::io::poll::Pollable)> = p
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|e| (i, &e.pollable)))
            .collect();

        if active.is_empty() {
            return Vec::new();
        }

        let pollables: Vec<&wasi::io::poll::Pollable> =
            active.iter().map(|(_, poll)| *poll).collect();
        let ready_indices = wasi_poll(&pollables);

        ready_indices
            .iter()
            .map(|&idx| active[idx as usize].0 as u32)
            .collect()
    })
}

/// Resolve ready handles: take entries from registry and call JS
/// `_isola_async._resolve()`. Returns the taken entries paired with their
/// handles for the caller to process, or resolves them directly via JS if a
/// context is provided.
pub fn resolve_ready(ctx: &Ctx<'_>, ready_handles: &[u32]) -> rquickjs::Result<()> {
    // Build array of ready handles for JS
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

/// Retrieve the result of a completed HTTP operation as a JS response object.
pub fn recv_http<'js>(ctx: &Ctx<'js>, handle: u32) -> rquickjs::Result<Object<'js>> {
    let op = take(handle)?;
    match op {
        PendingOp::Http { response, url } => {
            let response = response
                .get()
                .expect("response not ready")
                .expect("wasm error")
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message("fetch", "error", &e.to_string())
                })?;
            super::http::build_response_object(ctx, response, &url)
                .map_err(|e| rquickjs::Error::new_from_js_message("fetch", "error", &e))
        }
        _ => Err(rquickjs::Error::new_from_js_message(
            "recv",
            "error",
            "handle is not an HTTP operation",
        )),
    }
}

/// Retrieve the result of a completed hostcall as a JS value.
pub fn finish_hostcall<'js>(ctx: &Ctx<'js>, handle: u32) -> rquickjs::Result<Value<'js>> {
    let op = take(handle)?;
    match op {
        PendingOp::Hostcall(future) => {
            let result = future
                .get()
                .expect("hostcall not ready")
                .expect("wasm error");
            let cbor_result = result.map_err(|e| {
                rquickjs::Error::new_from_js_message("hostcall", "error", &e.to_debug_string())
            })?;
            js_serde::cbor_to_js(ctx, &cbor_result)
                .map_err(|e| rquickjs::Error::new_from_js_message("cbor", "value", &e))
        }
        _ => Err(rquickjs::Error::new_from_js_message(
            "hostcall",
            "error",
            "handle is not a hostcall operation",
        )),
    }
}

/// Consume a completed sleep handle (no result value).
pub fn finish_sleep(handle: u32) -> rquickjs::Result<()> {
    let op = take(handle)?;
    if matches!(op, PendingOp::Sleep) {
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

    // _isola_sys._drain_jobs - no-op, kept for compatibility
    sys.set(
        "_drain_jobs",
        rquickjs::Function::new(ctx.clone(), || {}).unwrap(),
    )
    .unwrap();

    // _isola_sys._finish_sleep(handle) -> void
    // Consumes a completed sleep handle.
    sys.set(
        "_finish_sleep",
        rquickjs::Function::new(ctx.clone(), finish_sleep).unwrap(),
    )
    .unwrap();
}
