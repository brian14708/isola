pub mod future;
pub mod http;
mod logging;
mod serde;

use std::{cell::RefCell, time::Instant};

pub use isola_runtime::{exports, isola, wasi};

use self::{exports::isola::script::runtime, isola::script::host};
use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde as js_serde,
};

#[cfg(target_arch = "wasm32")]
isola_runtime::export!(Global with_types_in isola_runtime);

pub struct Global;

impl runtime::Guest for Global {
    fn initialize(preinit: bool, prelude: Option<String>) {
        GLOBAL_SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            if scope.is_none() {
                const ASYNC_JS: &str = include_str!("../../js/sandbox/async.js");
                const WINTERTC_ABORT_JS: &str = include_str!("../../js/sandbox/wintertc_abort.js");
                const WINTERTC_HTTP_JS: &str = include_str!("../../js/sandbox/wintertc_http.js");

                let s = Scope::new();

                // Register native bridge modules as globals
                s.context().with(|ctx| {
                    self::serde::register(&ctx);
                    self::logging::register(&ctx);
                    self::http::register(&ctx);
                    register_sys_module(&ctx);
                    // future::register_js must come after register_sys_module
                    // because it reads _isola_sys from globals
                    self::future::register_js(&ctx);
                });

                // Load JS-side async infrastructure and HTTP platform wrappers.
                // async.js must come before wintertc_http.js (uses _isola_async._wait).
                // async.js must come after register_sys_module because it
                // reads _isola_sys and exposes top-level async helpers.
                s.load_script(ASYNC_JS).unwrap();
                s.load_script(WINTERTC_ABORT_JS).unwrap();
                s.load_script(WINTERTC_HTTP_JS).unwrap();

                if let Some(prelude) = prelude {
                    s.load_script(&prelude).unwrap();
                }
                scope.replace(s);
            }
        });

        if preinit {
            #[link(wasm_import_module = "wasi_snapshot_preview1")]
            unsafe extern "C" {
                #[cfg_attr(target_arch = "wasm32", link_name = "reset_adapter_state")]
                fn reset_adapter_state();
            }

            #[link(wasm_import_module = "env")]
            unsafe extern "C" {
                #[cfg_attr(target_arch = "wasm32", link_name = "__wasilibc_reset_preopens")]
                fn wasilibc_reset_preopens();
            }

            unsafe {
                reset_adapter_state();
                wasilibc_reset_preopens();
            }

            // A monotonic base captured during initialize() (snapshot/build
            // time) would be frozen into the Wizer snapshot and is meaningless
            // at runtime, so clear it here; `monotonic()` lazily establishes the
            // base on its first runtime call instead (see MONOTONIC_BASE).
            MONOTONIC_BASE.with(|base| {
                base.borrow_mut().take();
            });
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "WIT async export requires an async trait method"
    )]
    async fn eval_script(script: String) -> Result<(), runtime::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::Unexpected("Sandbox not initialized").into()),
                |sandbox| {
                    sandbox
                        .load_script(&script)
                        .map_err(Into::<runtime::Error>::into)
                },
            )
        })
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "WIT async export requires an async trait method"
    )]
    async fn eval_file(path: String) -> Result<(), runtime::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::Unexpected("Sandbox not initialized").into()),
                |sandbox| {
                    sandbox
                        .load_file(&path)
                        .map_err(Into::<runtime::Error>::into)
                },
            )
        })
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "WIT async export requires an async trait method"
    )]
    async fn call_func(func: String, args: Vec<runtime::Argument>) -> Result<(), runtime::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::Unexpected("Sandbox not initialized").into()),
                |sandbox| {
                    let mut positional = vec![];
                    let mut named = vec![];
                    for arg in args {
                        let runtime::Argument { name, value } = arg;
                        let value = match value {
                            isola::script::host::Value::Cbor(s) => InputValue::Cbor(s.into()),
                            isola::script::host::Value::CborIterator(e) => {
                                InputValue::Iter(collect_stream_arg(&e))
                            }
                        };
                        if let Some(name) = name {
                            named.push((name.into(), value));
                        } else {
                            positional.push(value);
                        }
                    }
                    sandbox
                        .run(&func, positional, named, |emit_type, data| {
                            isola::script::host::blocking_emit(emit_type, data);
                        })
                        .map_err(Into::<runtime::Error>::into)
                },
            )
        })
    }
}

fn collect_stream_arg(iter: &host::ValueIterator) -> Vec<Vec<u8>> {
    let mut items = Vec::new();
    while let Some(cbor) = isola_runtime::block_on(iter.read()) {
        items.push(cbor);
    }
    items
}

fn register_sys_module(ctx: &rquickjs::Ctx<'_>) {
    let globals = ctx.globals();

    let sys = rquickjs::Object::new(ctx.clone()).unwrap();

    // _isola_sys.emit(obj) - emit a partial result
    sys.set(
        "emit",
        rquickjs::Function::new(ctx.clone(), js_sys_emit).unwrap(),
    )
    .unwrap();

    // _isola_sys.hostcall(type, payload) -> handle: u32
    // Returns a pollable handle. Use with _isola_async._wait() +
    // _finish_hostcall().
    sys.set(
        "hostcall",
        rquickjs::Function::new(ctx.clone(), js_sys_hostcall).unwrap(),
    )
    .unwrap();

    // _isola_sys._finish_hostcall(handle) -> value
    // Retrieves the result of a completed hostcall.
    sys.set(
        "_finish_hostcall",
        rquickjs::Function::new(ctx.clone(), js_sys_finish_hostcall).unwrap(),
    )
    .unwrap();

    // _isola_sys.monotonic() - monotonic clock in seconds
    sys.set(
        "monotonic",
        rquickjs::Function::new(ctx.clone(), || -> f64 {
            MONOTONIC_BASE.with(|base| {
                base.borrow_mut()
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    .as_secs_f64()
            })
        })
        .unwrap(),
    )
    .unwrap();

    // _isola_sys.sleep(duration_secs) -> handle: u32
    // Returns a pollable handle. Use with _isola_async._wait().
    sys.set(
        "sleep",
        rquickjs::Function::new(ctx.clone(), |duration: f64| -> u32 {
            future::register(future::sleep(duration))
        })
        .unwrap(),
    )
    .unwrap();

    globals.set("_isola_sys", sys).unwrap();
}

fn js_sys_emit<'js>(_ctx: rquickjs::Ctx<'js>, val: rquickjs::Value<'js>) -> rquickjs::Result<()> {
    js_serde::js_to_cbor_emit(
        val,
        isola::script::host::EmitType::PartialResult,
        isola::script::host::blocking_emit,
    )
    .map_err(|e| rquickjs::Error::new_from_js_message("value", "cbor", &e))?;
    Ok(())
}

/// Submit a hostcall (non-blocking). Returns a pollable handle; the call is
/// driven later by the poll loop, concurrently with any other pending calls.
fn js_sys_hostcall(
    _ctx: rquickjs::Ctx<'_>,
    call_type: String,
    payload: rquickjs::Value<'_>,
) -> rquickjs::Result<u32> {
    let cbor_payload = js_serde::js_to_cbor(payload)
        .map_err(|e| rquickjs::Error::new_from_js_message("value", "cbor", &e))?;
    Ok(future::register(future::hostcall(call_type, cbor_payload)))
}

/// Retrieve the result of a completed hostcall.
#[expect(clippy::needless_pass_by_value)]
fn js_sys_finish_hostcall(
    ctx: rquickjs::Ctx<'_>,
    handle: u32,
) -> rquickjs::Result<rquickjs::Value<'_>> {
    future::finish_hostcall(&ctx, handle)
}

thread_local! {
    static GLOBAL_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
    static MONOTONIC_BASE: RefCell<Option<Instant>> = const { RefCell::new(None) };
}
