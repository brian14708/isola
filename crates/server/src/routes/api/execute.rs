use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::header,
    response::{IntoResponse, Response, Sse, sse::Event},
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use isola::{TRACE_TARGET_SCRIPT, cbor, trace::collect::CollectSpanExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{Span, info_span, level_filters::LevelFilter};

use crate::routes::{AppState, Argument, SandboxEnv, Source, StreamItem};

use super::{
    error::HttpApiError,
    trace::{HttpTrace, HttpTraceCollector},
    types::{
        ErrorCode, ExecuteRequest, ExecuteResponse, SseDataEvent, SseDoneEvent, SseErrorEvent,
    },
};

fn convert_args(
    args: &[serde_json::Value],
    kwargs: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(Option<String>, Argument)>, HttpApiError> {
    let mut result = Vec::with_capacity(args.len() + kwargs.len());

    for arg in args {
        let json_str = serde_json::to_string(arg)
            .map_err(|e| HttpApiError::invalid_request(format!("Failed to serialize arg: {e}")))?;
        let cbor = cbor::json_to_cbor(&json_str).map_err(|e| {
            HttpApiError::invalid_request(format!("Failed to convert to CBOR: {e}"))
        })?;
        result.push((None, Argument::cbor(cbor)));
    }

    for (name, value) in kwargs {
        let json_str = serde_json::to_string(value).map_err(|e| {
            HttpApiError::invalid_request(format!("Failed to serialize kwarg {name}: {e}"))
        })?;
        let cbor = cbor::json_to_cbor(&json_str).map_err(|e| {
            HttpApiError::invalid_request(format!("Failed to convert kwarg {name} to CBOR: {e}"))
        })?;
        result.push((Some(name.clone()), Argument::cbor(cbor)));
    }

    Ok(result)
}

fn cbor_to_json(data: &Bytes) -> Result<serde_json::Value, HttpApiError> {
    let json_str = cbor::cbor_to_json(data)
        .map_err(|e| HttpApiError::internal(format!("Failed to convert CBOR to JSON: {e}")))?;
    serde_json::from_str(&json_str)
        .map_err(|e| HttpApiError::internal(format!("Failed to parse JSON: {e}")))
}

fn map_start_error(err: anyhow::Error) -> HttpApiError {
    match err.downcast::<isola::Error>() {
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

    let (span, mut trace_rx, log_level) = if req.trace {
        let (collector, rx) = HttpTraceCollector::new();
        let s = info_span!(
            target: TRACE_TARGET_SCRIPT,
            parent: Span::current(),
            "script.exec"
        );
        if s.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector)
            .is_some()
        {
            (s, Some(rx), LevelFilter::DEBUG)
        } else {
            (Span::none(), None, LevelFilter::DEBUG)
        }
    } else {
        (Span::none(), None, LevelFilter::OFF)
    };

    let env = SandboxEnv {
        client: state.base_env.client.clone(),
        log_level,
    };

    let result = async {
        let _enter = span.enter();

        let cache_key = if req.trace { "trace" } else { "default" };
        let mut stream = state
            .sandbox_manager
            .exec(cache_key, source, req.function, args, timeout, env)
            .await
            .map_err(map_start_error)?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut final_result: Option<serde_json::Value> = None;

        while let Some(item) = stream.next().await {
            match item {
                StreamItem::Data(data) => {
                    let value = cbor_to_json(&data)?;
                    results.push(value);
                }
                StreamItem::End(Some(data)) => {
                    final_result = Some(cbor_to_json(&data)?);
                }
                StreamItem::End(None) => {}
                StreamItem::Error(err) => {
                    return Err(HttpApiError::from(err));
                }
            }
        }

        #[allow(clippy::option_if_let_else)]
        let result = if let Some(val) = final_result {
            val
        } else if results.len() == 1 {
            results.remove(0)
        } else {
            serde_json::Value::Array(results)
        };

        Ok::<_, HttpApiError>(result)
    };

    let result = match tokio::time::timeout(timeout, result).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(HttpApiError::timeout(req.timeout_ms)),
    };

    let mut traces = Vec::new();
    if let Some(ref mut rx) = trace_rx {
        while let Ok(trace) = rx.try_recv() {
            traces.push(trace);
        }
    }

    Ok(Json(ExecuteResponse { result, traces }))
}

enum SseItem {
    Trace(HttpTrace),
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

        let (span, trace_rx, log_level) = if req.trace {
            let (collector, rx) = HttpTraceCollector::new();
            let s = info_span!(
                target: TRACE_TARGET_SCRIPT,
                parent: Span::current(),
                "script.exec"
            );
            if s.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector)
                .is_some()
            {
                (s, Some(rx), LevelFilter::DEBUG)
            } else {
                (Span::none(), None, LevelFilter::DEBUG)
            }
        } else {
            (Span::none(), None, LevelFilter::OFF)
        };

        let env = SandboxEnv {
            client: state.base_env.client.clone(),
            log_level,
        };

        let _enter = span.enter();

        let cache_key = if req.trace { "trace" } else { "default" };
        let stream_result = state
            .sandbox_manager
            .exec(cache_key, source, req.function.clone(), args, timeout, env)
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

        let (merged_tx, mut merged_rx) = mpsc::unbounded_channel::<SseItem>();

        let trace_tx = merged_tx.clone();
        let trace_handle = trace_rx.map(|trace_rx| {
            tokio::spawn(async move {
                let mut rx = trace_rx;
                while let Some(trace) = rx.recv().await {
                    if trace_tx.send(SseItem::Trace(trace)).is_err() {
                        break;
                    }
                }
            })
        });

        let deadline = tokio::time::Instant::now() + timeout;
        let mut pending_traces = Vec::new();

        loop {
            tokio::select! {
                biased;

                () = tokio::time::sleep_until(deadline) => {
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
                            match cbor_to_json(&data) {
                                Ok(value) => {
                                    let event = SseDataEvent { value };
                                    if !emit_event(&tx, make_json_event("data", event)) {
                                        break;
                                    }
                                }
                                Err(e) => {
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
                            match cbor_to_json(&data) {
                                Ok(value) => {
                                    let event = SseDataEvent { value };
                                    if !emit_event(&tx, make_json_event("data", event)) {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let err = SseErrorEvent {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = emit_event(&tx, make_json_event("error", err));
                                    break;
                                }
                            }
                            while let Ok(SseItem::Trace(t)) = merged_rx.try_recv() {
                                pending_traces.push(t);
                            }
                            let done = SseDoneEvent { traces: pending_traces };
                            let _ = emit_event(&tx, make_json_event("done", done));
                            break;
                        }
                        Some(StreamItem::Error(err)) => {
                            let api_err = HttpApiError::from(err);
                            let err = SseErrorEvent {
                                code: api_err.code,
                                message: api_err.message,
                            };
                            let _ = emit_event(&tx, make_json_event("error", err));
                            break;
                        }
                        Some(StreamItem::End(None)) | None => {
                            while let Ok(SseItem::Trace(t)) = merged_rx.try_recv() {
                                pending_traces.push(t);
                            }
                            let done = SseDoneEvent { traces: pending_traces };
                            let _ = emit_event(&tx, make_json_event("done", done));
                            break;
                        }
                    }
                }

                Some(SseItem::Trace(trace)) = merged_rx.recv() => {
                    if !emit_event(&tx, make_json_event("trace", trace)) {
                        break;
                    }
                }
            }
        }

        if let Some(handle) = trace_handle {
            handle.abort();
        }
    });

    UnboundedReceiverStream::new(rx)
}
