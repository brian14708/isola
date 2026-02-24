use std::time::Duration;

use axum::{
    Json,
    extract::State,
    response::{Sse, sse::Event},
};
use futures::{Stream, StreamExt};
use isola::value::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::Instrument;

use super::{
    error::HttpApiError,
    trace::{HttpTraceBuilder, SCRIPT_EXEC_SPAN_NAME},
    types::{
        ErrorCode, ExecuteRequest, ExecuteResponse, SseDataEvent, SseDoneEvent, SseErrorEvent,
        SseLogEvent,
    },
};
use crate::routes::{AppState, Argument, ExecOptions, Runtime, SandboxEnv, Source, StreamItem};

fn convert_args(
    args: &[serde_json::Value],
    kwargs: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(Option<String>, Argument)>, HttpApiError> {
    let mut result = Vec::with_capacity(args.len() + kwargs.len());

    for arg in args {
        let value = Value::from_json_value(arg).map_err(|e| {
            HttpApiError::invalid_request(format!("Failed to convert to Value: {e}"))
        })?;
        result.push((None, Argument::value(value)));
    }

    for (name, value) in kwargs {
        let value = Value::from_json_value(value).map_err(|e| {
            HttpApiError::invalid_request(format!("Failed to convert kwarg {name} to Value: {e}"))
        })?;
        result.push((Some(name.clone()), Argument::value(value)));
    }

    Ok(result)
}

fn value_to_json(data: &Value) -> Result<serde_json::Value, HttpApiError> {
    data.to_json_value()
        .map_err(|e| HttpApiError::internal(format!("Failed to convert Value to JSON: {e}")))
}

fn map_start_error(err: anyhow::Error) -> HttpApiError {
    match err.downcast::<isola::sandbox::Error>() {
        Ok(err) => HttpApiError::from(err),
        Err(err) => HttpApiError::invalid_request(format!("Failed to start script: {err}")),
    }
}

fn resolve_runtime_manager(
    state: &AppState,
    runtime: Runtime,
) -> Result<std::sync::Arc<crate::routes::SandboxManager<SandboxEnv>>, HttpApiError> {
    state
        .runtime_factory
        .manager_for(runtime)
        .map_err(|err| HttpApiError::invalid_request(err.to_string()))
}

fn emit_event(
    tx: &mpsc::UnboundedSender<Result<Event, std::convert::Infallible>>,
    event: Event,
) -> bool {
    tx.send(Ok(event)).is_ok()
}

fn emit_json_event<T: serde::Serialize>(
    tx: &mpsc::UnboundedSender<Result<Event, std::convert::Infallible>>,
    event: &str,
    payload: &T,
) -> bool {
    match serde_json::to_string(payload) {
        Ok(data) => emit_event(tx, Event::default().event(event).data(data)),
        Err(err) => {
            tracing::error!(?err, %event, "failed to serialize SSE event payload");
            let fallback = serde_json::json!({
                "code": ErrorCode::Internal,
                "message": "Failed to serialize server event",
            });
            let _ = emit_event(
                tx,
                Event::default().event("error").data(fallback.to_string()),
            );
            false
        }
    }
}

#[utoipa::path(
    post,
    path = "/v1/execute",
    request_body = crate::routes::api::openapi::OpenApiExecuteRequest,
    responses(
        (status = 200, description = "Execution completed", body = crate::routes::api::openapi::OpenApiExecuteResponse),
        (status = 400, description = "Invalid request", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 408, description = "Execution timed out", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 422, description = "Script error", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 499, description = "Execution cancelled", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 500, description = "Internal error", body = crate::routes::api::openapi::OpenApiErrorResponse),
    ),
    params(
        ("traceparent" = Option<String>, Header, description = "W3C trace context"),
        ("tracestate" = Option<String>, Header, description = "W3C trace state"),
    ),
    operation_id = "executeSyncV1",
    tag = "Execute"
)]
#[tracing::instrument(
    target = "isola_server::script",
    skip(state, req),
    fields(transport = "sync")
)]
pub async fn execute_sync(
    State(state): State<AppState>,
    Json(req): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, HttpApiError> {
    let runtime_manager = resolve_runtime_manager(&state, req.runtime)?;
    let args = convert_args(&req.args, &req.kwargs)?;
    let source = Source {
        prelude: req.prelude,
        code: req.script,
    };
    let timeout = Duration::from_millis(req.timeout_ms);

    let env = SandboxEnv {
        client: state.base_env.client.clone(),
        request_proxy: state.base_env.request_proxy.clone(),
    };

    let result = async {
        let cache_key = if req.trace { "trace" } else { "default" };
        let mut stream = runtime_manager
            .exec(
                cache_key,
                source,
                req.function,
                args,
                env,
                ExecOptions { timeout },
            )
            .await
            .map_err(map_start_error)?;

        let trace_builder = req.trace.then(HttpTraceBuilder::new);
        let mut traces = Vec::new();
        let span_parent_id = trace_builder.as_ref().map(|builder| {
            let trace = builder.span_begin(SCRIPT_EXEC_SPAN_NAME);
            let parent_id = trace.id;
            traces.push(trace);
            parent_id
        });

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut final_result: Option<serde_json::Value> = None;

        while let Some(item) = stream.next().await {
            match item {
                StreamItem::Data(data) => {
                    let value = value_to_json(&data)?;
                    results.push(value);
                }
                StreamItem::End(Some(data)) => {
                    final_result = Some(value_to_json(&data)?);
                }
                StreamItem::End(None) => {}
                StreamItem::Log {
                    level,
                    context: _context,
                    message,
                } => {
                    if let Some(builder) = trace_builder.as_ref() {
                        traces.push(builder.log(&level, message));
                    }
                }
                StreamItem::Error(err) => {
                    return Err(HttpApiError::from(err));
                }
            }
        }

        if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
            traces.push(builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id));
        }

        #[allow(clippy::option_if_let_else)]
        let result = if let Some(val) = final_result {
            val
        } else if results.len() == 1 {
            results.remove(0)
        } else {
            serde_json::Value::Array(results)
        };

        Ok::<_, HttpApiError>((result, traces))
    };

    let (result, traces) = match tokio::time::timeout(timeout, result).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(HttpApiError::timeout(req.timeout_ms)),
    };

    Ok(Json(ExecuteResponse { result, traces }))
}

#[utoipa::path(
    post,
    path = "/v1/execute/stream",
    request_body = crate::routes::api::openapi::OpenApiExecuteRequest,
    responses(
        (
            status = 200,
            description = "SSE stream with event types: data, log, trace, error, done",
            body = String,
            content_type = "text/event-stream"
        ),
        (status = 400, description = "Invalid request", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 408, description = "Execution timed out", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 422, description = "Script error", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 499, description = "Execution cancelled", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 500, description = "Internal error", body = crate::routes::api::openapi::OpenApiErrorResponse),
    ),
    params(
        ("traceparent" = Option<String>, Header, description = "W3C trace context"),
        ("tracestate" = Option<String>, Header, description = "W3C trace state"),
    ),
    operation_id = "executeStreamV1",
    tag = "Execute"
)]
#[tracing::instrument(
    target = "isola_server::script",
    skip(state, req),
    fields(transport = "sse")
)]
pub async fn execute_stream(
    State(state): State<AppState>,
    Json(req): Json<ExecuteRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, HttpApiError> {
    let ExecuteRequest {
        script,
        prelude,
        runtime,
        function,
        args,
        kwargs,
        timeout_ms,
        trace,
    } = req;

    let args = convert_args(&args, &kwargs)?;
    let source = Source {
        prelude,
        code: script,
    };
    let timeout = Duration::from_millis(timeout_ms);

    Ok(Sse::new(execute_sse_inner(
        state,
        ExecuteSseInput {
            runtime,
            source,
            function,
            args,
            timeout,
            timeout_ms,
            trace,
        },
    )))
}

struct ExecuteSseInput {
    runtime: Runtime,
    source: Source,
    function: String,
    args: Vec<(Option<String>, Argument)>,
    timeout: Duration,
    timeout_ms: u64,
    trace: bool,
}

#[allow(clippy::too_many_lines)]
fn execute_sse_inner(
    state: AppState,
    input: ExecuteSseInput,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    let ExecuteSseInput {
        runtime,
        source,
        function,
        args,
        timeout,
        timeout_ms,
        trace,
    } = input;

    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, std::convert::Infallible>>();

    tokio::spawn(async move {
        let env = SandboxEnv {
            client: state.base_env.client.clone(),
            request_proxy: state.base_env.request_proxy.clone(),
        };
        let runtime_manager = match resolve_runtime_manager(&state, runtime) {
            Ok(manager) => manager,
            Err(err) => {
                let payload = SseErrorEvent {
                    code: err.code,
                    message: err.message,
                };
                let _ = emit_json_event(&tx, "error", &payload);
                return;
            }
        };

        let cache_key = if trace { "trace" } else { "default" };
        let stream_result = runtime_manager
            .exec(
                cache_key,
                source,
                function,
                args,
                env,
                ExecOptions { timeout },
            )
            .await;

        let mut data_stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                let mapped = map_start_error(e);
                let err = SseErrorEvent {
                    code: mapped.code,
                    message: mapped.message,
                };
                let _ = emit_json_event(&tx, "error", &err);
                return;
            }
        };

        let trace_builder = trace.then(HttpTraceBuilder::new);
        let mut pending_traces = Vec::new();
        let span_parent_id = if let Some(builder) = trace_builder.as_ref() {
            let trace_event = builder.span_begin(SCRIPT_EXEC_SPAN_NAME);
            if !emit_json_event(&tx, "trace", &trace_event) {
                return;
            }
            let parent_id = trace_event.id;
            pending_traces.push(trace_event);
            Some(parent_id)
        } else {
            None
        };

        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            tokio::select! {
                biased;

                () = tokio::time::sleep_until(deadline) => {
                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                        pending_traces.push(trace_event.clone());
                        if !emit_json_event(&tx, "trace", &trace_event) {
                            break;
                        }
                    }
                    let err = SseErrorEvent {
                        code: ErrorCode::Timeout,
                        message: format!("Execution timed out after {timeout_ms}ms"),
                    };
                    let _ = emit_json_event(&tx, "error", &err);
                    break;
                }

                item = data_stream.next() => {
                    match item {
                        Some(StreamItem::Data(data)) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let event = SseDataEvent { value };
                                    if !emit_json_event(&tx, "data", &event) {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if !emit_json_event(&tx, "trace", &trace_event) {
                                            break;
                                        }
                                    }
                                    let err = SseErrorEvent {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = emit_json_event(&tx, "error", &err);
                                    break;
                                }
                            }
                        }
                        Some(StreamItem::End(Some(data))) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let event = SseDataEvent { value };
                                    if !emit_json_event(&tx, "data", &event) {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if !emit_json_event(&tx, "trace", &trace_event) {
                                            break;
                                        }
                                    }
                                    let err = SseErrorEvent {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = emit_json_event(&tx, "error", &err);
                                    break;
                                }
                            }

                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if !emit_json_event(&tx, "trace", &trace_event) {
                                    break;
                                }
                            }

                            let done = SseDoneEvent { traces: pending_traces };
                            let _ = emit_json_event(&tx, "done", &done);
                            break;
                        }
                        Some(StreamItem::Error(err)) => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if !emit_json_event(&tx, "trace", &trace_event) {
                                    break;
                                }
                            }
                            let api_err = HttpApiError::from(err);
                            let err = SseErrorEvent {
                                code: api_err.code,
                                message: api_err.message,
                            };
                            let _ = emit_json_event(&tx, "error", &err);
                            break;
                        }
                        Some(StreamItem::Log {
                            level,
                            context,
                            message,
                        }) => {
                            let event = SseLogEvent {
                                level: level.clone(),
                                context,
                                message: message.clone(),
                            };
                            if !emit_json_event(&tx, "log", &event) {
                                break;
                            }
                            if let Some(builder) = trace_builder.as_ref() {
                                let trace_event = builder.log(&level, message);
                                pending_traces.push(trace_event.clone());
                                if !emit_json_event(&tx, "trace", &trace_event) {
                                    break;
                                }
                            }
                        }
                        Some(StreamItem::End(None)) | None => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if !emit_json_event(&tx, "trace", &trace_event) {
                                    break;
                                }
                            }
                            let done = SseDoneEvent { traces: pending_traces };
                            let _ = emit_json_event(&tx, "done", &done);
                            break;
                        }
                    }
                }
            }
        }
    }
    .instrument(tracing::Span::current()));

    UnboundedReceiverStream::new(rx)
}
