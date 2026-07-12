use isola_runtime::{
    Deadline,
    pending::{self, InvalidHandle, Output, Take},
    wasi_http::HttpRequest,
};
use rquickjs::{Array, Ctx, Function, Object, Value};

use crate::serde as js_serde;

pub fn register_hostcall(call_type: String, payload: Vec<u8>) -> u32 {
    pending::register_hostcall(call_type, payload)
}

pub fn register_http(request: HttpRequest) -> u32 {
    pending::register_http(request)
}

fn invalid_handle_error(error: InvalidHandle) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        "async",
        "handle",
        &format!("invalid or already-consumed handle: {}", error.handle()),
    )
}

pub fn drive_pending(mut step: impl FnMut(&[u32]) -> Drive) -> bool {
    pending::drive_pending(|| {
        let ready = pending::ready_handles();
        step(&ready)
    })
}

pub use isola_runtime::pending::Drive;

pub fn resolve_ready(ctx: &Ctx<'_>, ready_handles: &[u32]) -> rquickjs::Result<()> {
    let arr = Array::new(ctx.clone())?;
    for (i, &handle) in ready_handles.iter().enumerate() {
        arr.set(i, handle)?;
    }

    let globals = ctx.globals();
    let async_obj: Object<'_> = globals.get("_isola_async")?;
    let resolve_fn: Function<'_> = async_obj.get("_resolve")?;
    resolve_fn.call::<_, ()>((arr,))?;
    Ok(())
}

pub fn has_pending() -> bool {
    pending::has_pending()
}

pub fn cancel_all(ctx: &Ctx<'_>) -> rquickjs::Result<usize> {
    let globals = ctx.globals();
    let async_obj: Object<'_> = globals.get("_isola_async")?;
    let cancel_fn: Function<'_> = async_obj.get("_cancel_all")?;
    cancel_fn.call(())
}

pub fn discard_all(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let async_obj: Object<'_> = globals.get("_isola_async")?;
    let discard_fn: Function<'_> = async_obj.get("_discard_all")?;
    discard_fn.call(())
}

pub fn release(handle: u32) {
    pending::release(handle);
}

pub fn clear() {
    pending::clear();
}

pub fn recv_http<'js>(ctx: &Ctx<'js>, handle: u32) -> rquickjs::Result<Object<'js>> {
    match pending::take(handle).map_err(invalid_handle_error)? {
        Take::Ready(Output::Http {
            request_url,
            response,
        }) => {
            let response =
                response.map_err(|e| rquickjs::Error::new_from_js_message("fetch", "error", &e))?;
            super::http::build_response_object(ctx, response, &request_url)
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
    match pending::take(handle).map_err(invalid_handle_error)? {
        Take::Ready(Output::Host(result)) => {
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
    match pending::take(handle).map_err(invalid_handle_error)? {
        Take::Ready(Output::Sleep) => Ok(()),
        _ => Err(rquickjs::Error::new_from_js_message(
            "sleep",
            "error",
            "handle is not a sleep operation",
        )),
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

    sys.set(
        "_release",
        rquickjs::Function::new(ctx.clone(), release).unwrap(),
    )
    .unwrap();
}

pub fn register_sleep(duration: f64) -> rquickjs::Result<u32> {
    let deadline = Deadline::after_secs_f64(duration).map_err(|_| {
        rquickjs::Error::new_from_js_message("sleep", "duration", "sleep duration is out of range")
    })?;
    Ok(pending::register_sleep(deadline))
}
