use std::{collections::HashMap, time::Duration};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use isola::value::Value;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{
    error::HttpApiError,
    trace::{HttpTrace, HttpTraceBuilder, SCRIPT_EXEC_SPAN_NAME},
    types::ErrorCode,
};
use crate::routes::{AppState, Argument, ExecOptions, SandboxEnv, Source, StreamItem};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Init {
        runtime: String,
        script: String,
        #[serde(default)]
        prelude: String,
        #[serde(default = "default_function")]
        function: String,
        #[serde(default)]
        args: Vec<serde_json::Value>,
        #[serde(default)]
        kwargs: serde_json::Map<String, serde_json::Value>,
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
        #[serde(default)]
        trace: bool,
    },
    Push {
        stream: u32,
        value: serde_json::Value,
    },
    Close {
        stream: u32,
    },
    Cancel,
}

const fn default_timeout() -> u64 {
    30000
}

fn default_function() -> String {
    "main".to_string()
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Data {
        value: serde_json::Value,
    },
    Log {
        level: String,
        context: String,
        message: String,
    },
    Trace(HttpTrace),
    Done {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        traces: Vec<HttpTrace>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

fn make_text_message(msg: &ServerMessage) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap_or_default().into())
}

fn map_start_error(err: anyhow::Error) -> HttpApiError {
    match err.downcast::<isola::sandbox::Error>() {
        Ok(err) => HttpApiError::from(err),
        Err(err) => HttpApiError::invalid_request(format!("Failed to start script: {err}")),
    }
}

pub async fn ws_execute(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

#[allow(clippy::too_many_lines)]
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    loop {
        let init_msg = loop {
            match receiver.next().await {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text)
                {
                    Ok(ClientMessage::Init {
                        runtime,
                        script,
                        prelude,
                        function,
                        args,
                        kwargs,
                        timeout_ms,
                        trace,
                    }) => {
                        break Some((
                            runtime, script, prelude, function, args, kwargs, timeout_ms, trace,
                        ));
                    }
                    Ok(_) => {
                        let msg = ServerMessage::Error {
                            code: ErrorCode::InvalidRequest,
                            message: "Expected init message".to_string(),
                        };
                        if sender.send(make_text_message(&msg)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let msg = ServerMessage::Error {
                            code: ErrorCode::InvalidRequest,
                            message: format!("Invalid message: {e}"),
                        };
                        if sender.send(make_text_message(&msg)).await.is_err() {
                            return;
                        }
                    }
                },
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            }
        };

        let Some((runtime, script, prelude, function, args, kwargs, timeout_ms, trace)) = init_msg
        else {
            return;
        };

        if runtime != "python3" {
            let msg = ServerMessage::Error {
                code: ErrorCode::UnknownRuntime,
                message: format!("Unknown runtime: {runtime}"),
            };
            if sender.send(make_text_message(&msg)).await.is_err() {
                return;
            }
            continue;
        }

        let mut stream_channels: HashMap<u32, mpsc::Sender<Value>> = HashMap::new();
        let mut converted_args: Vec<(Option<String>, Argument)> = Vec::new();

        for arg in &args {
            if let Some(stream_id) = extract_stream_marker(arg) {
                let (tx, rx) = mpsc::channel(64);
                stream_channels.insert(stream_id, tx);
                converted_args.push((None, Argument::stream(ReceiverStream::new(rx))));
            } else {
                match json_to_value_arg(arg) {
                    Ok(value) => converted_args.push((None, Argument::value(value))),
                    Err(e) => {
                        let msg = ServerMessage::Error {
                            code: ErrorCode::InvalidRequest,
                            message: e.message,
                        };
                        if sender.send(make_text_message(&msg)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }

        for (name, value) in &kwargs {
            if let Some(stream_id) = extract_stream_marker(value) {
                let (tx, rx) = mpsc::channel(64);
                stream_channels.insert(stream_id, tx);
                converted_args.push((
                    Some(name.clone()),
                    Argument::stream(ReceiverStream::new(rx)),
                ));
            } else {
                match json_to_value_arg(value) {
                    Ok(v) => converted_args.push((Some(name.clone()), Argument::value(v))),
                    Err(e) => {
                        let msg = ServerMessage::Error {
                            code: ErrorCode::InvalidRequest,
                            message: e.message,
                        };
                        if sender.send(make_text_message(&msg)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }

        let source = Source {
            prelude,
            code: script,
        };
        let timeout = Duration::from_millis(timeout_ms);

        let env = SandboxEnv {
            client: state.base_env.client.clone(),
        };

        let cache_key = if trace { "trace" } else { "default" };
        let stream_result = state
            .sandbox_manager
            .exec(
                cache_key,
                source,
                function,
                converted_args,
                env,
                ExecOptions { timeout },
            )
            .await;

        let mut data_stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                let mapped = map_start_error(e);
                let msg = ServerMessage::Error {
                    code: mapped.code,
                    message: mapped.message,
                };
                if sender.send(make_text_message(&msg)).await.is_err() {
                    return;
                }
                continue;
            }
        };

        let trace_builder = trace.then(HttpTraceBuilder::new);
        let mut pending_traces = Vec::new();
        let span_parent_id = if let Some(builder) = trace_builder.as_ref() {
            let trace_event = builder.span_begin(SCRIPT_EXEC_SPAN_NAME);
            if sender
                .send(make_text_message(&ServerMessage::Trace(
                    trace_event.clone(),
                )))
                .await
                .is_err()
            {
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
                        if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                            return;
                        }
                    }
                    let msg = ServerMessage::Error {
                        code: ErrorCode::Timeout,
                        message: format!("Execution timed out after {timeout_ms}ms"),
                    };
                    let _ = sender.send(make_text_message(&msg)).await;
                    break;
                }

                client_msg = receiver.next() => {
                    match client_msg {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ClientMessage>(&text) {
                                Ok(ClientMessage::Cancel) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                            return;
                                        }
                                    }
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Cancelled,
                                        message: "Execution cancelled".to_string(),
                                    };
                                    let _ = sender.send(make_text_message(&msg)).await;
                                    break;
                                }
                                Ok(ClientMessage::Push { stream, value }) => {
                                    if let Some(tx) = stream_channels.get(&stream)
                                        && let Ok(v) = json_to_value_arg(&value)
                                    {
                                        let _ = tx.send(v).await;
                                    }
                                }
                                Ok(ClientMessage::Close { stream }) => {
                                    stream_channels.remove(&stream);
                                }
                                Ok(ClientMessage::Init { .. }) => {
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::InvalidRequest,
                                        message: "Execution already in progress".to_string(),
                                    };
                                    let _ = sender.send(make_text_message(&msg)).await;
                                }
                                Err(_) => {}
                            }
                        }
                        Some(Ok(Message::Close(_)) | Err(_)) | None => {
                            return;
                        }
                        Some(Ok(_)) => {}
                    }
                }

                item = data_stream.next() => {
                    match item {
                        Some(StreamItem::Data(data)) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let msg = ServerMessage::Data { value };
                                    if sender.send(make_text_message(&msg)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                            return;
                                        }
                                    }
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = sender.send(make_text_message(&msg)).await;
                                    break;
                                }
                            }
                        }
                        Some(StreamItem::End(Some(data))) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let msg = ServerMessage::Data { value };
                                    if sender.send(make_text_message(&msg)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                            return;
                                        }
                                    }
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = sender.send(make_text_message(&msg)).await;
                                    break;
                                }
                            }

                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                    return;
                                }
                            }

                            let msg = ServerMessage::Done {
                                traces: std::mem::take(&mut pending_traces),
                            };
                            let _ = sender.send(make_text_message(&msg)).await;
                            break;
                        }
                        Some(StreamItem::End(None)) | None => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                    return;
                                }
                            }
                            let msg = ServerMessage::Done {
                                traces: std::mem::take(&mut pending_traces),
                            };
                            let _ = sender.send(make_text_message(&msg)).await;
                            break;
                        }
                        Some(StreamItem::Log {
                            level,
                            context,
                            message,
                        }) => {
                            let msg = ServerMessage::Log {
                                level: level.clone(),
                                context: context.clone(),
                                message: message.clone(),
                            };
                            if sender.send(make_text_message(&msg)).await.is_err() {
                                return;
                            }

                            if let Some(builder) = trace_builder.as_ref() {
                                let trace_event = builder.log(&level, message);
                                pending_traces.push(trace_event.clone());
                                if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Some(StreamItem::Error(err)) => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if sender.send(make_text_message(&ServerMessage::Trace(trace_event))).await.is_err() {
                                    return;
                                }
                            }
                            let api_err = HttpApiError::from(err);
                            let msg = ServerMessage::Error {
                                code: api_err.code,
                                message: api_err.message,
                            };
                            let _ = sender.send(make_text_message(&msg)).await;
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn extract_stream_marker(value: &serde_json::Value) -> Option<u32> {
    value
        .as_object()
        .and_then(|obj| obj.get("$stream"))
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as u32)
}

fn json_to_value_arg(value: &serde_json::Value) -> Result<Value, HttpApiError> {
    Value::from_json_value(value)
        .map_err(|e| HttpApiError::invalid_request(format!("Failed to convert to Value: {e}")))
}

fn value_to_json(data: &Value) -> Result<serde_json::Value, HttpApiError> {
    data.to_json_value()
        .map_err(|e| HttpApiError::internal(format!("Failed to convert Value to JSON: {e}")))
}
