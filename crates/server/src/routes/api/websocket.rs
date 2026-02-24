use std::{collections::HashMap, time::Duration};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderName, HeaderValue},
    response::Response,
};
use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use isola::value::Value;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

use super::{
    error::HttpApiError,
    trace::{HttpTrace, HttpTraceBuilder, SCRIPT_EXEC_SPAN_NAME},
    types::ErrorCode,
};
use crate::routes::{AppState, Argument, ExecOptions, Runtime, SandboxEnv, Source, StreamItem};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Init {
        runtime: Runtime,
        script: String,
        #[serde(default)]
        prelude: String,
        function: String,
        #[serde(default)]
        args: Vec<serde_json::Value>,
        #[serde(default)]
        kwargs: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        headers: HashMap<String, String>,
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

#[derive(Debug)]
struct InitPayload {
    runtime: Runtime,
    script: String,
    prelude: String,
    function: String,
    args: Vec<serde_json::Value>,
    kwargs: serde_json::Map<String, serde_json::Value>,
    headers: HashMap<String, String>,
    timeout_ms: u64,
    trace: bool,
}

const fn default_timeout() -> u64 {
    30000
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

fn convert_ws_args(
    args: &[serde_json::Value],
    kwargs: &serde_json::Map<String, serde_json::Value>,
    stream_channels: &mut HashMap<u32, mpsc::Sender<Value>>,
) -> Result<Vec<(Option<String>, Argument)>, HttpApiError> {
    let mut converted_args = Vec::with_capacity(args.len() + kwargs.len());

    for arg in args {
        if let Some(stream_id) = extract_stream_marker(arg) {
            let (tx, rx) = mpsc::channel(64);
            stream_channels.insert(stream_id, tx);
            converted_args.push((None, Argument::stream(ReceiverStream::new(rx))));
        } else {
            let value = json_to_value_arg(arg)?;
            converted_args.push((None, Argument::value(value)));
        }
    }

    for (name, value) in kwargs {
        if let Some(stream_id) = extract_stream_marker(value) {
            let (tx, rx) = mpsc::channel(64);
            stream_channels.insert(stream_id, tx);
            converted_args.push((
                Some(name.clone()),
                Argument::stream(ReceiverStream::new(rx)),
            ));
        } else {
            let value = json_to_value_arg(value)?;
            converted_args.push((Some(name.clone()), Argument::value(value)));
        }
    }

    Ok(converted_args)
}

fn parse_init_headers(headers: &HashMap<String, String>) -> Result<HeaderMap, HttpApiError> {
    let mut result = HeaderMap::new();

    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| HttpApiError::invalid_request(format!("Invalid header name: {name}")))?;
        let header_value = HeaderValue::from_str(value).map_err(|_| {
            HttpApiError::invalid_request(format!("Invalid header value for header: {name}"))
        })?;
        result.insert(header_name, header_value);
    }

    Ok(result)
}

async fn send_server_message(
    sender: &mut SplitSink<WebSocket, Message>,
    msg: &ServerMessage,
) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => sender.send(Message::Text(text.into())).await.is_ok(),
        Err(err) => {
            tracing::error!(?err, "failed to serialize websocket message");
            let fallback = serde_json::json!({
                "type": "error",
                "code": ErrorCode::Internal,
                "message": "Failed to serialize server message",
            })
            .to_string();
            let _ = sender.send(Message::Text(fallback.into())).await;
            false
        }
    }
}

#[utoipa::path(
    get,
    path = "/v1/execute/ws",
    responses(
        (status = 101, description = "WebSocket protocol upgrade completed"),
        (status = 400, description = "Bad request", body = crate::routes::api::openapi::OpenApiErrorResponse),
        (status = 500, description = "Internal error", body = crate::routes::api::openapi::OpenApiErrorResponse),
    ),
    operation_id = "executeWsV1",
    tag = "Execute"
)]
#[tracing::instrument(
    target = "isola_server::script",
    skip(ws, state),
    fields(transport = "ws")
)]
pub async fn ws_execute(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

enum SocketAction {
    Continue,
    Close,
}

#[allow(clippy::too_many_lines)]
async fn run_init_execution(
    sender: &mut SplitSink<WebSocket, Message>,
    receiver: &mut SplitStream<WebSocket>,
    state: &AppState,
    init: InitPayload,
) -> SocketAction {
    tracing::info!(
        runtime = init.runtime.as_str(),
        function = %init.function,
        timeout_ms = init.timeout_ms,
        trace = init.trace,
        arg_count = init.args.len(),
        kwarg_count = init.kwargs.len(),
        header_count = init.headers.len(),
        "Received websocket execution init"
    );

    let parent_headers = match parse_init_headers(&init.headers) {
        Ok(headers) => headers,
        Err(e) => {
            let msg = ServerMessage::Error {
                code: ErrorCode::InvalidRequest,
                message: e.message,
            };
            return if send_server_message(sender, &msg).await {
                SocketAction::Continue
            } else {
                SocketAction::Close
            };
        }
    };

    let span = tracing::span!(
        target: "isola_server::script",
        tracing::Level::INFO,
        "ws.execute",
        transport = "ws",
        phase = "execution"
    );
    let parent_context = super::trace_context::extract_parent_context(&parent_headers);
    super::trace_context::attach_parent_context(&span, parent_context);

    async move {
        let function_name = init.function.clone();
        let mut stream_channels: HashMap<u32, mpsc::Sender<Value>> = HashMap::new();
        let converted_args = match convert_ws_args(&init.args, &init.kwargs, &mut stream_channels) {
            Ok(args) => args,
            Err(e) => {
                let msg = ServerMessage::Error {
                    code: ErrorCode::InvalidRequest,
                    message: e.message,
                };
                return if send_server_message(sender, &msg).await {
                    SocketAction::Continue
                } else {
                    SocketAction::Close
                };
            }
        };

        let source = Source {
            prelude: init.prelude,
            code: init.script,
        };
        let timeout = Duration::from_millis(init.timeout_ms);

        let env = SandboxEnv {
            client: state.base_env.client.clone(),
            request_proxy: state.base_env.request_proxy.clone(),
        };
        let runtime_manager = match resolve_runtime_manager(state, init.runtime) {
            Ok(manager) => manager,
            Err(err) => {
                let msg = ServerMessage::Error {
                    code: err.code,
                    message: err.message,
                };
                return if send_server_message(sender, &msg).await {
                    SocketAction::Continue
                } else {
                    SocketAction::Close
                };
            }
        };

        let cache_key = if init.trace { "trace" } else { "default" };
        let stream_result = runtime_manager
            .exec(
                cache_key,
                source,
                init.function,
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
                return if send_server_message(sender, &msg).await {
                    SocketAction::Continue
                } else {
                    SocketAction::Close
                };
            }
        };

        let trace_builder = init.trace.then(HttpTraceBuilder::new);
        let mut pending_traces = Vec::new();
        let span_parent_id = if let Some(builder) = trace_builder.as_ref() {
            let trace_event = builder.span_begin(SCRIPT_EXEC_SPAN_NAME);
            if !send_server_message(sender, &ServerMessage::Trace(trace_event.clone())).await {
                return SocketAction::Close;
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
                    tracing::warn!(
                        function = %function_name,
                        timeout_ms = init.timeout_ms,
                        "Websocket execution timed out"
                    );
                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                        pending_traces.push(trace_event.clone());
                        if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                            return SocketAction::Close;
                        }
                    }
                    let msg = ServerMessage::Error {
                        code: ErrorCode::Timeout,
                        message: format!("Execution timed out after {}ms", init.timeout_ms),
                    };
                    let _ = send_server_message(sender, &msg).await;
                    return SocketAction::Continue;
                }

                client_msg = receiver.next() => {
                    match client_msg {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ClientMessage>(&text) {
                                Ok(ClientMessage::Cancel) => {
                                    tracing::info!(function = %function_name, "Websocket execution cancelled by client");
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                            return SocketAction::Close;
                                        }
                                    }
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Cancelled,
                                        message: "Execution cancelled".to_string(),
                                    };
                                    let _ = send_server_message(sender, &msg).await;
                                    return SocketAction::Continue;
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
                                    let _ = send_server_message(sender, &msg).await;
                                }
                                Err(_) => {}
                            }
                        }
                        Some(Ok(Message::Close(_)) | Err(_)) | None => {
                            return SocketAction::Close;
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
                                    if !send_server_message(sender, &msg).await {
                                        return SocketAction::Close;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                            return SocketAction::Close;
                                        }
                                    }
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = send_server_message(sender, &msg).await;
                                    return SocketAction::Continue;
                                }
                            }
                        }
                        Some(StreamItem::End(Some(data))) => {
                            match value_to_json(&data) {
                                Ok(value) => {
                                    let msg = ServerMessage::Data { value };
                                    if !send_server_message(sender, &msg).await {
                                        return SocketAction::Close;
                                    }
                                }
                                Err(e) => {
                                    if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                        let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                        pending_traces.push(trace_event.clone());
                                        if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                            return SocketAction::Close;
                                        }
                                    }
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = send_server_message(sender, &msg).await;
                                    return SocketAction::Continue;
                                }
                            }

                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                    return SocketAction::Close;
                                }
                            }

                            let msg = ServerMessage::Done {
                                traces: std::mem::take(&mut pending_traces),
                            };
                            let _ = send_server_message(sender, &msg).await;
                            tracing::info!(function = %function_name, "Websocket execution completed");
                            return SocketAction::Continue;
                        }
                        Some(StreamItem::End(None)) | None => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                    return SocketAction::Close;
                                }
                            }
                            let msg = ServerMessage::Done {
                                traces: std::mem::take(&mut pending_traces),
                            };
                            let _ = send_server_message(sender, &msg).await;
                            tracing::info!(function = %function_name, "Websocket execution completed");
                            return SocketAction::Continue;
                        }
                        Some(StreamItem::Log {
                            level,
                            context,
                            message,
                        }) => {
                            let msg = ServerMessage::Log {
                                level: level.clone(),
                                context,
                                message: message.clone(),
                            };
                            if !send_server_message(sender, &msg).await {
                                return SocketAction::Close;
                            }

                            if let Some(builder) = trace_builder.as_ref() {
                                let trace_event = builder.log(&level, message);
                                pending_traces.push(trace_event.clone());
                                if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                    return SocketAction::Close;
                                }
                            }
                        }
                        Some(StreamItem::Error(err)) => {
                            if let (Some(builder), Some(parent_id)) = (trace_builder.as_ref(), span_parent_id) {
                                let trace_event = builder.span_end(SCRIPT_EXEC_SPAN_NAME, parent_id);
                                pending_traces.push(trace_event.clone());
                                if !send_server_message(sender, &ServerMessage::Trace(trace_event)).await {
                                    return SocketAction::Close;
                                }
                            }
                            let api_err = HttpApiError::from(err);
                            let msg = ServerMessage::Error {
                                code: api_err.code,
                                message: api_err.message,
                            };
                            let _ = send_server_message(sender, &msg).await;
                            return SocketAction::Continue;
                        }
                    }
                }
            }
        }
    }
    .instrument(span)
    .await
}

#[allow(clippy::too_many_lines)]
#[tracing::instrument(
    target = "isola_server::script",
    skip(socket, state),
    fields(transport = "ws", phase = "session")
)]
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
                        headers,
                        timeout_ms,
                        trace,
                    }) => {
                        break Some(InitPayload {
                            runtime,
                            script,
                            prelude,
                            function,
                            args,
                            kwargs,
                            headers,
                            timeout_ms,
                            trace,
                        });
                    }
                    Ok(_) => {
                        let msg = ServerMessage::Error {
                            code: ErrorCode::InvalidRequest,
                            message: "Expected init message".to_string(),
                        };
                        if !send_server_message(&mut sender, &msg).await {
                            return;
                        }
                    }
                    Err(e) => {
                        let msg = ServerMessage::Error {
                            code: ErrorCode::InvalidRequest,
                            message: format!("Invalid message: {e}"),
                        };
                        if !send_server_message(&mut sender, &msg).await {
                            return;
                        }
                    }
                },
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            }
        };

        let Some(init) = init_msg else {
            return;
        };
        if matches!(
            run_init_execution(&mut sender, &mut receiver, &state, init).await,
            SocketAction::Close
        ) {
            return;
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
