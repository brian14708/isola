use std::borrow::Cow;

use rquickjs::{Array, Context, Ctx, Function, Object, Runtime, Value, function::Args};

use crate::{
    error::{Error, Result},
    serde::{cbor_to_js, js_to_cbor_emit},
    wasm::{future, isola::script::host::EmitType},
};

pub struct Scope {
    #[allow(dead_code)] // Runtime must be kept alive for the context to function
    runtime: Runtime,
    context: Context,
}

pub enum InputValue<'a> {
    Cbor(Cow<'a, [u8]>),
    Iter(Vec<Vec<u8>>),
}

impl Scope {
    fn input_to_js<'js>(ctx: &Ctx<'js>, input: InputValue<'_>) -> Result<Value<'js>> {
        match input {
            InputValue::Cbor(cbor) => cbor_to_js(ctx, cbor.as_ref()).map_err(|e| Error::JsError {
                cause: e,
                stack: None,
            }),
            InputValue::Iter(items) => {
                let arr = Array::new(ctx.clone()).map_err(|_| Error::from_js_catch(ctx))?;
                for (index, item) in items.into_iter().enumerate() {
                    let value = cbor_to_js(ctx, &item).map_err(|e| Error::JsError {
                        cause: e,
                        stack: None,
                    })?;
                    arr.set(index, value)
                        .map_err(|_| Error::from_js_catch(ctx))?;
                }
                Ok(arr.into_value())
            }
        }
    }

    pub fn new() -> Self {
        let runtime = Runtime::new().expect("failed to create QuickJS runtime");
        runtime.set_max_stack_size(2 * 1024 * 1024); // 2MB stack

        let context = Context::full(&runtime).expect("failed to create QuickJS context");

        Self { runtime, context }
    }

    pub const fn context(&self) -> &Context {
        &self.context
    }

    pub fn load_script(&self, code: &str) -> Result<()> {
        self.context.with(|ctx| {
            ctx.eval::<(), _>(code)
                .map_err(|_| Error::from_js_catch(&ctx))
        })
    }

    pub fn load_file(&self, path: &str) -> Result<()> {
        let code = std::fs::read_to_string(path)
            .map_err(|_| Error::UnexpectedError("failed to read script"))?;
        self.context.with(|ctx| {
            ctx.eval::<(), _>(code.as_str())
                .map_err(|_| Error::from_js_catch(&ctx))
        })
    }

    pub fn run<'a>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue<'a>>,
        named: impl IntoIterator<Item = (Cow<'a, str>, InputValue<'a>)>,
        mut callback: impl FnMut(EmitType, &[u8]),
    ) -> Result<()> {
        self.context.with(|ctx| {
            let globals = ctx.globals();
            let val: Value<'_> = globals.get(name).map_err(|_| Error::from_js_catch(&ctx))?;

            let obj = if val.is_function() {
                let func = val
                    .as_function()
                    .ok_or(Error::UnexpectedError("expected function"))?;

                // Build arguments
                let mut args: Vec<Value<'_>> = Vec::new();
                for v in positional {
                    args.push(Self::input_to_js(&ctx, v)?);
                }

                // Collect named args into a final options object
                let named_items: Vec<_> = named.into_iter().collect();
                if !named_items.is_empty() {
                    let opts = Object::new(ctx.clone()).map_err(|_| Error::from_js_catch(&ctx))?;
                    for (k, v) in named_items {
                        let val = Self::input_to_js(&ctx, v)?;
                        opts.set(k.as_ref(), val)
                            .map_err(|_| Error::from_js_catch(&ctx))?;
                    }
                    args.push(opts.into_value());
                }

                self.call_function(func, &ctx, args)?
            } else {
                val
            };

            self.emit_result(&ctx, obj, &mut callback)
        })
    }

    /// Drive a Promise to completion using the WASI poll-based event loop.
    ///
    /// 1. Execute microtasks (`execute_pending_job`)
    /// 2. If promise unresolved and pending WASI ops exist → call
    ///    `wasi:io/poll::poll`
    /// 3. Call JS `_isola_async._resolve(readyHandles)` to resolve
    ///    corresponding Promises
    /// 4. New microtasks are created → repeat
    #[allow(clippy::unused_self)]
    fn drive_promise<'js>(
        &self,
        ctx: &Ctx<'js>,
        promise: &rquickjs::Promise<'js>,
    ) -> Result<Value<'js>> {
        loop {
            // Check if promise already resolved/rejected
            match promise.result::<Value<'js>>() {
                Some(Ok(val)) => return Ok(val),
                Some(Err(_)) => return Err(Error::from_js_catch(ctx)),
                None => {}
            }

            // Drive microtask queue until fully drained.
            // Use ctx.execute_pending_job() (not runtime) to avoid RefCell double-borrow
            // since we're inside context.with().
            while ctx.execute_pending_job() {
                // Re-check promise after each job
                if promise.result::<Value<'js>>().is_some() {
                    break;
                }
            }

            // Re-check after draining all microtasks
            if promise.result::<Value<'js>>().is_some() {
                continue;
            }

            // No microtasks remaining — poll pending WASI I/O
            if !future::has_pending() {
                return Err(Error::UnexpectedError("promise never resolved"));
            }

            let ready_handles = future::poll_all();
            if ready_handles.is_empty() {
                return Err(Error::UnexpectedError("poll returned no ready handles"));
            }

            // Call JS _isola_async._resolve(readyHandles)
            future::resolve_ready(ctx, &ready_handles).map_err(|_| Error::from_js_catch(ctx))?;
        }
    }

    fn call_function<'js>(
        &self,
        func: &Function<'js>,
        ctx: &Ctx<'js>,
        args: Vec<Value<'js>>,
    ) -> Result<Value<'js>> {
        let mut js_args = Args::new(ctx.clone(), args.len());
        for arg in args {
            js_args
                .push_arg(arg)
                .map_err(|_| Error::from_js_catch(ctx))?;
        }
        let result: Value<'js> = func
            .call_arg(js_args)
            .map_err(|_| Error::from_js_catch(ctx))?;

        // Check if result is a Promise
        if result.is_promise() {
            let promise = result
                .as_promise()
                .ok_or(Error::UnexpectedError("expected promise"))?;

            self.drive_promise(ctx, promise)
        } else {
            Ok(result)
        }
    }

    #[allow(clippy::too_many_lines)]
    fn emit_result<'js>(
        &self,
        ctx: &Ctx<'js>,
        obj: Value<'js>,
        callback: &mut impl FnMut(EmitType, &[u8]),
    ) -> Result<()> {
        // Check if it's an async generator (has Symbol.asyncIterator) BEFORE sync
        // generator check, because async generators also have .next() but
        // return Promises.
        if let Some(gen_obj) = obj.as_object() {
            let check_async_iter: std::result::Result<Function<'_>, _> = ctx.eval(
                "(function(o) { return typeof o[Symbol.asyncIterator] === 'function' ? o[Symbol.asyncIterator]() : null; })",
            );
            if let Ok(check_fn) = check_async_iter
                && let Ok(iter) = check_fn.call::<_, Value<'_>>((gen_obj.clone(),))
                && iter.is_object()
                && !iter.is_null()
            {
                return self.iterate_async_generator(ctx, &iter, callback);
            }
        }

        // Check if it's a sync generator (has next() method)
        if let Some(gen_obj) = obj.as_object()
            && gen_obj.get::<_, Function<'_>>("next").is_ok()
        {
            // Use a JS helper to call next() with proper `this` binding
            let call_next: Function<'_> = ctx
                .eval("(function(g) { return g.next(); })")
                .map_err(|_| Error::from_js_catch(ctx))?;

            // It's a generator - iterate it
            loop {
                let iter_result: Object<'_> = call_next
                    .call((gen_obj.clone(),))
                    .map_err(|_| Error::from_js_catch(ctx))?;
                let done: bool = iter_result.get("done").unwrap_or(false);
                let value: Value<'_> = iter_result
                    .get("value")
                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));

                if done {
                    // Final value from generator - if not undefined, emit as end
                    if value.is_undefined() {
                        callback(EmitType::End, &[]);
                    } else {
                        js_to_cbor_emit(value, EmitType::End, callback).map_err(|e| {
                            Error::JsError {
                                cause: e,
                                stack: None,
                            }
                        })?;
                    }
                    return Ok(());
                }

                js_to_cbor_emit(value, EmitType::PartialResult, &mut *callback).map_err(|e| {
                    Error::JsError {
                        cause: e,
                        stack: None,
                    }
                })?;
            }
        }

        // Regular serializable value
        if Self::is_serializable(&obj) {
            return js_to_cbor_emit(obj, EmitType::End, callback).map_err(|e| Error::JsError {
                cause: e,
                stack: None,
            });
        }

        // Try Symbol.iterator
        if let Some(iter_obj) = obj.as_object() {
            let get_iter: std::result::Result<Function<'_>, _> = ctx
                .eval("(function(o) { return typeof o[Symbol.iterator] === 'function' ? o[Symbol.iterator]() : null; })");
            if let Ok(get_iter) = get_iter
                && let Ok(iter) = get_iter.call::<_, Value<'_>>((iter_obj.clone(),))
                && let Some(iter_val) = iter.as_object()
                && iter_val.get::<_, Function<'_>>("next").is_ok()
            {
                let call_next: Function<'_> = ctx
                    .eval("(function(g) { return g.next(); })")
                    .map_err(|_| Error::from_js_catch(ctx))?;
                loop {
                    let iter_result: Object<'_> = call_next
                        .call((iter_val.clone(),))
                        .map_err(|_| Error::from_js_catch(ctx))?;
                    let done: bool = iter_result.get("done").unwrap_or(false);
                    if done {
                        callback(EmitType::End, &[]);
                        return Ok(());
                    }
                    let value: Value<'_> = iter_result
                        .get("value")
                        .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                    js_to_cbor_emit(value, EmitType::PartialResult, &mut *callback).map_err(
                        |e| Error::JsError {
                            cause: e,
                            stack: None,
                        },
                    )?;
                }
            }
        }

        Err(Error::UnexpectedError(
            "Return type is not serializable or iterable",
        ))
    }

    /// Iterate an async generator to completion, emitting each yielded value.
    fn iterate_async_generator<'js>(
        &self,
        ctx: &Ctx<'js>,
        iter: &Value<'js>,
        callback: &mut impl FnMut(EmitType, &[u8]),
    ) -> Result<()> {
        let call_next: Function<'_> = ctx
            .eval("(function(g) { return g.next(); })")
            .map_err(|_| Error::from_js_catch(ctx))?;

        loop {
            // Each call to next() returns a Promise<{value, done}>
            let next_result: Value<'_> = call_next
                .call((iter.clone(),))
                .map_err(|_| Error::from_js_catch(ctx))?;

            // Drive the promise to completion
            let iter_result = if next_result.is_promise() {
                let promise = next_result.as_promise().ok_or(Error::UnexpectedError(
                    "expected promise from async generator",
                ))?;
                self.drive_promise(ctx, promise)?
            } else {
                next_result
            };

            let iter_obj: Object<'_> = iter_result
                .as_object()
                .cloned()
                .ok_or(Error::UnexpectedError("expected object from next()"))?;
            let done: bool = iter_obj.get("done").unwrap_or(false);
            let value: Value<'_> = iter_obj
                .get("value")
                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));

            if done {
                if value.is_undefined() {
                    callback(EmitType::End, &[]);
                } else {
                    js_to_cbor_emit(value, EmitType::End, callback).map_err(|e| {
                        Error::JsError {
                            cause: e,
                            stack: None,
                        }
                    })?;
                }
                return Ok(());
            }

            js_to_cbor_emit(value, EmitType::PartialResult, &mut *callback).map_err(|e| {
                Error::JsError {
                    cause: e,
                    stack: None,
                }
            })?;
        }
    }

    fn is_serializable(val: &Value<'_>) -> bool {
        val.is_null()
            || val.is_undefined()
            || val.is_bool()
            || val.is_int()
            || val.is_number()
            || val.is_string()
            || val.is_array()
            || val.is_object()
    }
}
