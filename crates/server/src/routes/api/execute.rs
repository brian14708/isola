use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::header,
    response::{IntoResponse, Response, Sse, sse::Event},
};
use futures::{Stream, StreamExt};
use isola::value::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::{
    error::HttpApiError,
    trace::{HttpTraceBuilder, SCRIPT_EXEC_SPAN_NAME},
    types::{
        ErrorCode, ExecuteRequest, ExecuteResponse, SseDataEvent, SseDoneEvent, SseErrorEvent,
        SseLogEvent,
    },
};
use crate::routes::{AppState, Argument, ExecOptions, SandboxEnv, Source, StreamItem};

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

fn make_json_event<T: serde::Serialize>(event: &str, payload: T) -> Event {
    Event::default()
        .event(event)
        .json_data(payload)
        .unwrap_or_else(|_| Event::default())
}

fn emit_event(
    tx: &mpsc::UnboundedSender<Result<Event, std::convert::Infallible>>,
    event: Event,
) -> bool {
    tx.send(Ok(event)).is_ok()
}

pub async fn execute(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ExecuteRequest>,
) -> Response {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");

    if accept.contains("text/event-stream") {
        execute_sse(state, req).await.into_response()
    } else {
        execute_json(state, req).await.into_response()
    }
}

async fn execute_json(
    state: AppState,
    req: ExecuteRequest,
) -> Result<Json<ExecuteResponse>, HttpApiError> {
    if req.runtime != "python3" {
        return Err(HttpApiError::unknown_runtime(&req.runtime));
    }

    let args = convert_args(&req.args, &req.kwargs)?;
    let source = Source {
        prelude: req.prelude,
        code: req.script,
    };
    let timeout = Duration::from_millis(req.timeout_ms);

    let env = SandboxEnv {
        client: state.base_env.client.clone(),
    };

    let result = async {
        let cache_key = if req.trace { "trace" } else { "default" };
        let mut stream = state
            .sandbox_manager
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

#[allow(clippy::unused_async)]
async fn execute_sse(
    state: AppState,
    req: ExecuteRequest,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    Sse::new(execute_sse_inner(state, req))
}

#[allow(clippy::too_many_lines)]
fn execute_sse_inner(
    state: AppState,
    req: ExecuteRequest,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, std::convert::Infallible>>();

    tokio::spawn(async move {
        if req.runtime != "python3" {
            let err = SseErrorEvent {
                code: ErrorCode::UnknownRuntime,
                message: format!("Unknown runtime: {}", req.runtime),
            };
            let _ = emit_event(&tx, make_json_event("error", err));
            return;
        }

        let args = match convert_args(&req.args, &req.kwargs) {
            Ok(a) => a,
            Err(e) => {
                let err = SseErrorEvent {
                    code: e.code,
                    message: e.message,
                };
                let _ = emit_event(&tx, make_json_event("error", err));
                return;
            }
        };

        let source = Source {
            prelude: req.prelude.clone(),
            code: req.script.clone(),
        };
        let timeout = Duration::from_millis(req.timeout_ms);

        let env = SandboxEnv {
            client: state.base_env.client.clone(),
        };

        let cache_key = if req.trace { "trace" } else { "default" };
        let stream_result = state
            .sandbox_manager
            .exec(
                cache_key,
                source,
                req.function.clone(),
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
                let _ = emit_event(&tx, make_json_event("error", err));
                return;
            }
        };

        let trace_builder = req.trace.then(HttpTraceBuilder::new);
        let mut pending_traces = Vec::new();
        let span_parent_id = if let Some(builder) = trace_builder.as_ref() {
            let trace = builder.span_begin(SCRIPT_EXEC_SPAN_NAME);
            if !emit_event(&tx, make_json_event("trace", trace.clone())) {
                return;
            }
            let parent_id = trace.id;
            pending_traces.push(trace);
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
                        let trace = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                        pending_traces.push(trace.clone());
                        if !emit_event(&tx, make_json_event("trace", trace)) {
                            break;
                        }
                    }
                    let err = SseErrorEvent {
                        code: ErrorCode::Timeout,
                        message: format!("Execution timed out after {}ms", req.timeout_ms),
                    };
                    let _ = emit_event(&tx, make_json_event("error", err));
                    break;
                }

                item = data_stream.next() => {
                    match item {
                        Some(StreamItem::Data(data)) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let event = SseDataEvent { value };
                                    if !emit_event(&tx, make_json_event("data", event)) {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace.clone());
                                        if !emit_event(&tx, make_json_event("trace", trace)) {
                                            break;
                                        }
                                    }
                                    let err = SseErrorEvent {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = emit_event(&tx, make_json_event("error", err));
                                    break;
                                }
                            }
                        }
                        Some(StreamItem::End(Some(data))) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let event = SseDataEvent { value };
                                    if !emit_event(&tx, make_json_event("data", event)) {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace.clone());
                                        if !emit_event(&tx, make_json_event("trace", trace)) {
                                            break;
                                        }
                                    }
                                    let err = SseErrorEvent {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = emit_event(&tx, make_json_event("error", err));
                                    break;
                                }
                            }

                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace.clone());
                                if !emit_event(&tx, make_json_event("trace", trace)) {
                                    break;
                                }
                            }

                            let done = SseDoneEvent { traces: pending_traces };
                            let _ = emit_event(&tx, make_json_event("done", done));
                            break;
                        }
                        Some(StreamItem::Error(err)) => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace.clone());
                                if !emit_event(&tx, make_json_event("trace", trace)) {
                                    break;
                                }
                            }
                            let api_err = HttpApiError::from(err);
                            let err = SseErrorEvent {
                                code: api_err.code,
                                message: api_err.message,
                            };
                            let _ = emit_event(&tx, make_json_event("error", err));
                            break;
                        }
                        Some(StreamItem::Log {
                            level,
                            context,
                            message,
                        }) => {
                            let event = SseLogEvent {
                                level: level.clone(),
                                context: context.clone(),
                                message: message.clone(),
                            };
                            if !emit_event(&tx, make_json_event("log", event)) {
                                break;
                            }
                            if let Some(builder) = trace_builder.as_ref() {
                                let trace = builder.log(&level, message);
                                pending_traces.push(trace.clone());
                                if !emit_event(&tx, make_json_event("trace", trace)) {
                                    break;
                                }
                            }
                        }
                        Some(StreamItem::End(None)) | None => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace.clone());
                                if !emit_event(&tx, make_json_event("trace", trace)) {
                                    break;
                                }
                            }
                            let done = SseDoneEvent { traces: pending_traces };
                            let _ = emit_event(&tx, make_json_event("done", done));
                            break;
                        }
                    }
                }
            }
        }
    });

    UnboundedReceiverStream::new(rx)
}
