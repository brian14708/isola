use std::sync::Arc;

use async_trait::async_trait;
use isola::{
    host::{BoxError, LogContext, LogLevel, OutputSink},
    sandbox::{Arg, Sandbox},
    value::Value,
};
use napi::{
    bindgen_prelude::{Buffer, Function, Promise},
    threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;
use parking_lot::Mutex;

use crate::{
    context::{ContextInner, PendingSandboxConfig, SandboxConfigPatch},
    env::{Env, JsHostcallHandler, JsHttpHandler},
    error::{Error, invalid_argument},
    stream::StreamHandle,
};

// Type alias for the callback TSFN, wrapped in Arc for cloning.
// Type params: T, Return, CallJsBackArgs, ErrorStatus, CalleeHandled
type CallbackTsfn = Arc<
    ThreadsafeFunction<(String, Option<String>), (), (String, Option<String>), napi::Status, false>,
>;

// ---------------------------------------------------------------------------
// RunResult
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct RunResult {
    pub result_json: Vec<String>,
    pub final_json: Option<String>,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    pub logs: Vec<String>,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// OutputCollector (implements OutputSink)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum CallbackEvent {
    Result,
    End,
    Stdout,
    Stderr,
    Error,
    Log,
}

impl CallbackEvent {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Result => "result",
            Self::End => "end",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Error => "error",
            Self::Log => "log",
        }
    }
}

#[derive(Default, Clone)]
struct OutputData {
    result_json: Vec<String>,
    final_json: Option<String>,
    stdout: Vec<String>,
    stderr: Vec<String>,
    logs: Vec<String>,
    errors: Vec<String>,
}

#[derive(Clone)]
struct OutputCollector {
    callback: Option<CallbackTsfn>,
    data: Arc<Mutex<OutputData>>,
}

impl OutputCollector {
    fn new(callback: Option<CallbackTsfn>) -> Self {
        Self {
            callback,
            data: Arc::new(Mutex::new(OutputData::default())),
        }
    }

    fn record<F>(&self, f: F)
    where
        F: FnOnce(&mut OutputData),
    {
        let mut data = self.data.lock();
        f(&mut data);
    }

    fn emit(&self, event: CallbackEvent, payload: Option<&str>) {
        if let Some(tsfn) = &self.callback {
            tsfn.call(
                (event.as_str().to_owned(), payload.map(str::to_owned)),
                ThreadsafeFunctionCallMode::NonBlocking,
            );
        }
    }

    fn emit_error_message(&self, message: &str) {
        self.record(|data| data.errors.push(message.to_owned()));
        self.emit(CallbackEvent::Error, Some(message));
    }

    fn into_result(self) -> RunResult {
        let data = self.data.lock().clone();
        RunResult {
            result_json: data.result_json,
            final_json: data.final_json,
            stdout: data.stdout,
            stderr: data.stderr,
            logs: data.logs,
            errors: data.errors,
        }
    }
}

#[async_trait]
impl OutputSink for OutputCollector {
    async fn on_item(&self, item: Value) -> std::result::Result<(), BoxError> {
        let text = item.to_json_str().map_err(|e| -> BoxError {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
        self.record(|data| data.result_json.push(text));
        let json_str = item.to_json_str().ok();
        self.emit(CallbackEvent::Result, json_str.as_deref());
        Ok(())
    }

    async fn on_complete(&self, item: Option<Value>) -> std::result::Result<(), BoxError> {
        if let Some(item) = item {
            let text = item.to_json_str().map_err(|e| -> BoxError {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
            self.record(|data| data.final_json = Some(text));
            let json_str = item.to_json_str().ok();
            self.emit(CallbackEvent::End, json_str.as_deref());
        } else {
            self.emit(CallbackEvent::End, None);
        }
        Ok(())
    }

    async fn on_log(
        &self,
        level: LogLevel,
        _log_context: LogContext<'_>,
        message: &str,
    ) -> std::result::Result<(), BoxError> {
        match level {
            LogLevel::Stdout => {
                self.record(|data| data.stdout.push(message.to_string()));
                self.emit(CallbackEvent::Stdout, Some(message));
            }
            LogLevel::Stderr => {
                self.record(|data| data.stderr.push(message.to_string()));
                self.emit(CallbackEvent::Stderr, Some(message));
            }
            _ => {
                self.record(|data| data.logs.push(message.to_string()));
                self.emit(CallbackEvent::Log, Some(message));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SandboxInner state machine
// ---------------------------------------------------------------------------

enum SandboxInner {
    Uninitialized,
    Pending {
        config: PendingSandboxConfig,
        callback: Option<CallbackTsfn>,
        http_handler: Option<Arc<JsHttpHandler>>,
        hostcall_handler: Option<Arc<JsHostcallHandler>>,
    },
    Running {
        sandbox: Option<Sandbox<Env>>,
        callback: Option<CallbackTsfn>,
    },
}

// ---------------------------------------------------------------------------
// RunningSandboxLease
// ---------------------------------------------------------------------------

struct RunningSandboxLease {
    inner: Arc<Mutex<SandboxInner>>,
    sandbox: Option<Sandbox<Env>>,
}

impl RunningSandboxLease {
    const fn new(inner: Arc<Mutex<SandboxInner>>, sandbox: Sandbox<Env>) -> Self {
        Self {
            inner,
            sandbox: Some(sandbox),
        }
    }

    const fn sandbox_mut(&mut self) -> &mut Sandbox<Env> {
        self.sandbox
            .as_mut()
            .expect("running sandbox lease must contain sandbox")
    }
}

impl Drop for RunningSandboxLease {
    fn drop(&mut self) {
        let Some(sandbox) = self.sandbox.take() else {
            return;
        };
        let mut guard = self.inner.lock();
        if let SandboxInner::Running { sandbox: slot, .. } = &mut *guard
            && slot.is_none()
        {
            *slot = Some(sandbox);
        }
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

type WireArgument = (String, Option<String>, serde_json::Value);

enum RawArgument {
    Json(Option<String>, Value),
    #[allow(dead_code)]
    JsonStream(Option<String>, tokio::sync::mpsc::Receiver<Value>),
}

fn parse_run_args(args: Vec<WireArgument>) -> crate::error::Result<Vec<RawArgument>> {
    let mut parsed = Vec::with_capacity(args.len());
    for (kind, name, payload) in args {
        match kind.as_str() {
            "json" => {
                let value = Value::from_serde(&payload)
                    .map_err(|e| invalid_argument(format!("invalid argument value: {e}")))?;
                parsed.push(RawArgument::Json(name, value));
            }
            "stream" => {
                return Err(invalid_argument(
                    "stream arguments must use runWithStream method",
                ));
            }
            _ => {
                return Err(invalid_argument(format!(
                    "unsupported argument kind: {kind}"
                )));
            }
        }
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Helper: take lease from running sandbox
// ---------------------------------------------------------------------------

fn take_running_lease(
    inner: &Arc<Mutex<SandboxInner>>,
) -> napi::Result<(RunningSandboxLease, Option<CallbackTsfn>)> {
    let mut guard = inner.lock();
    match &mut *guard {
        SandboxInner::Running { sandbox, callback } => {
            let sandbox = sandbox
                .take()
                .ok_or_else(|| napi::Error::from(invalid_argument("sandbox is busy")))?;
            Ok((
                RunningSandboxLease::new(Arc::clone(inner), sandbox),
                callback.clone(),
            ))
        }
        _ => Err(napi::Error::from(invalid_argument(
            "sandbox is not running",
        ))),
    }
}

// ---------------------------------------------------------------------------
// N-API class: SandboxCore
// ---------------------------------------------------------------------------

#[napi]
pub struct SandboxCore {
    ctx: Arc<ContextInner>,
    inner: Arc<Mutex<SandboxInner>>,
}

impl SandboxCore {
    pub(crate) fn new(ctx: Arc<ContextInner>) -> Self {
        Self {
            ctx,
            inner: Arc::new(Mutex::new(SandboxInner::Pending {
                config: PendingSandboxConfig::default(),
                callback: None,
                http_handler: None,
                hostcall_handler: None,
            })),
        }
    }
}

#[napi]
impl SandboxCore {
    #[napi]
    pub fn configure(&self, config: serde_json::Value) -> napi::Result<()> {
        let patch: SandboxConfigPatch = serde_json::from_value(config)
            .map_err(|e| napi::Error::from(invalid_argument(format!("invalid config: {e}"))))?;
        let mut guard = self.inner.lock();
        match &mut *guard {
            SandboxInner::Pending { config, .. } => {
                config.apply_patch(patch).map_err(napi::Error::from)
            }
            SandboxInner::Running { .. } => Err(napi::Error::from(invalid_argument(
                "sandbox is already running",
            ))),
            SandboxInner::Uninitialized => Err(napi::Error::from(invalid_argument(
                "sandbox is not initialized",
            ))),
        }
    }

    /// Set the output event callback: (kind: string, data: string | null) =>
    /// void
    #[napi(ts_args_type = "callback: ((kind: string, data: string | null) => void) | null")]
    pub fn set_callback(
        &self,
        callback: Option<Function<(String, Option<String>), ()>>,
    ) -> napi::Result<()> {
        let tsfn: Option<CallbackTsfn> = callback
            .map(|cb| -> napi::Result<CallbackTsfn> {
                Ok(Arc::new(cb.build_threadsafe_function().build()?))
            })
            .transpose()?;

        let mut guard = self.inner.lock();
        match &mut *guard {
            SandboxInner::Pending { callback: slot, .. }
            | SandboxInner::Running { callback: slot, .. } => {
                *slot = tsfn;
                Ok(())
            }
            SandboxInner::Uninitialized => Err(napi::Error::from(invalid_argument(
                "sandbox is not initialized",
            ))),
        }
    }

    /// Set the hostcall handler: (callType: string, payloadJson: string) =>
    /// Promise<string>
    #[napi(
        ts_args_type = "handler: ((callType: string, payloadJson: string) => Promise<string>) | null"
    )]
    pub fn set_hostcall_handler(
        &self,
        handler: Option<Function<(String, String), Promise<String>>>,
    ) -> napi::Result<()> {
        let js_handler = handler
            .map(|cb| {
                let tsfn = cb.build_threadsafe_function().build()?;
                Ok::<_, napi::Error>(Arc::new(JsHostcallHandler::new(tsfn)))
            })
            .transpose()?;

        let mut guard = self.inner.lock();
        match &mut *guard {
            SandboxInner::Pending {
                hostcall_handler: slot,
                ..
            } => {
                *slot = js_handler;
                Ok(())
            }
            SandboxInner::Running { .. } => Err(napi::Error::from(invalid_argument(
                "sandbox is already running",
            ))),
            SandboxInner::Uninitialized => Err(napi::Error::from(invalid_argument(
                "sandbox is not initialized",
            ))),
        }
    }

    /// Set the HTTP handler: (method, url, headersJson, body) =>
    /// Promise<response>
    #[napi(
        ts_args_type = "handler: ((method: string, url: string, headersJson: string, body: Buffer | null) => Promise<{ status: number; headers?: Record<string, string>; body?: Buffer | null }>) | null"
    )]
    #[allow(clippy::type_complexity)]
    pub fn set_http_handler(
        &self,
        handler: Option<
            Function<(String, String, String, Option<Buffer>), Promise<crate::env::JsHttpResponse>>,
        >,
    ) -> napi::Result<()> {
        let js_handler = handler
            .map(|cb| {
                let tsfn = cb.build_threadsafe_function().build()?;
                Ok::<_, napi::Error>(Arc::new(JsHttpHandler::new(tsfn)))
            })
            .transpose()?;

        let mut guard = self.inner.lock();
        match &mut *guard {
            SandboxInner::Pending {
                http_handler: slot, ..
            } => {
                *slot = js_handler;
                Ok(())
            }
            SandboxInner::Running { .. } => Err(napi::Error::from(invalid_argument(
                "sandbox is already running",
            ))),
            SandboxInner::Uninitialized => Err(napi::Error::from(invalid_argument(
                "sandbox is not initialized",
            ))),
        }
    }

    #[napi]
    pub async fn start(&self) -> napi::Result<()> {
        let inner = Arc::clone(&self.inner);
        let ctx = Arc::clone(&self.ctx);

        let (config, callback, http_handler, hostcall_handler) = {
            let mut guard = inner.lock();
            let current = std::mem::replace(&mut *guard, SandboxInner::Uninitialized);
            match current {
                SandboxInner::Pending {
                    config,
                    callback,
                    http_handler,
                    hostcall_handler,
                } => (config, callback, http_handler, hostcall_handler),
                other => {
                    *guard = other;
                    drop(guard);
                    return Err(napi::Error::from(invalid_argument(
                        "sandbox is not in pending state",
                    )));
                }
            }
        };

        let options = config.to_options();
        let env = Env::new(http_handler.clone(), hostcall_handler.clone());

        match ctx.instantiate_sandbox(options, env).await {
            Ok(sandbox) => {
                let mut guard = inner.lock();
                *guard = SandboxInner::Running {
                    sandbox: Some(sandbox),
                    callback,
                };
                drop(guard);
                Ok(())
            }
            Err(err) => {
                let mut guard = inner.lock();
                *guard = SandboxInner::Pending {
                    config,
                    callback,
                    http_handler,
                    hostcall_handler,
                };
                drop(guard);
                Err(napi::Error::from(err))
            }
        }
    }

    #[napi]
    pub async fn load_script(&self, code: String) -> napi::Result<()> {
        let inner = Arc::clone(&self.inner);
        let (mut lease, callback) = take_running_lease(&inner)?;

        let collector = OutputCollector::new(callback);
        let sink = Arc::new(collector.clone());
        let outcome = lease.sandbox_mut().eval_script(&code, sink).await;
        if let Err(err) = outcome {
            let message = format!("Script loading failed: {err}");
            collector.emit_error_message(&message);
            return Err(napi::Error::from(Error::Internal(message)));
        }
        Ok(())
    }

    #[napi]
    pub async fn run(&self, func: String, args: Vec<WireArgument>) -> napi::Result<RunResult> {
        let inner = Arc::clone(&self.inner);
        let (mut lease, callback) = take_running_lease(&inner)?;

        let parsed_args = parse_run_args(args).map_err(napi::Error::from)?;

        let collector = OutputCollector::new(callback);
        let sink = Arc::new(collector.clone());
        let isola_args = parsed_args
            .into_iter()
            .map(|arg| match arg {
                RawArgument::Json(name, value) => Ok(match name {
                    Some(name) => Arg::Named(name, value),
                    None => Arg::Positional(value),
                }),
                RawArgument::JsonStream(name, receiver) => {
                    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver));
                    Ok(match name {
                        Some(name) => Arg::NamedStream(name, stream),
                        None => Arg::PositionalStream(stream),
                    })
                }
            })
            .collect::<crate::error::Result<Vec<_>>>()
            .map_err(napi::Error::from)?;

        let outcome = lease
            .sandbox_mut()
            .call_with_sink(&func, isola_args, sink)
            .await;
        if let Err(err) = outcome {
            let message = format!("Sandbox execution failed: {err}");
            collector.emit_error_message(&message);
            return Err(napi::Error::from(Error::Internal(message)));
        }

        Ok(collector.into_result())
    }

    /// Run a function with a `StreamHandle` argument.
    #[napi]
    pub async fn run_with_stream(
        &self,
        func: String,
        json_args: Vec<(Option<String>, serde_json::Value)>,
        stream_arg: &StreamHandle,
        stream_name: Option<String>,
    ) -> napi::Result<RunResult> {
        let inner = Arc::clone(&self.inner);
        let (mut lease, callback) = take_running_lease(&inner)?;

        let mut isola_args: Vec<Arg> = Vec::new();

        for (name, value) in json_args {
            let v = Value::from_serde(&value).map_err(|e| {
                napi::Error::from(invalid_argument(format!("invalid argument: {e}")))
            })?;
            isola_args.push(match name {
                Some(n) => Arg::Named(n, v),
                None => Arg::Positional(v),
            });
        }

        let receiver = stream_arg.take_receiver().map_err(napi::Error::from)?;
        let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver));
        isola_args.push(match stream_name {
            Some(n) => Arg::NamedStream(n, stream),
            None => Arg::PositionalStream(stream),
        });

        let collector = OutputCollector::new(callback);
        let sink = Arc::new(collector.clone());

        let outcome = lease
            .sandbox_mut()
            .call_with_sink(&func, isola_args, sink)
            .await;
        if let Err(err) = outcome {
            let message = format!("Sandbox execution failed: {err}");
            collector.emit_error_message(&message);
            return Err(napi::Error::from(Error::Internal(message)));
        }

        Ok(collector.into_result())
    }

    #[napi]
    pub fn close(&self) {
        let mut guard = self.inner.lock();
        *guard = SandboxInner::Uninitialized;
    }
}
