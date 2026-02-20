use std::{collections::HashMap, time::Duration};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use isola::{TRACE_TARGET_SCRIPT, cbor, trace::collect::CollectSpanExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{Span, info_span, level_filters::LevelFilter};

use crate::routes::{AppState, Argument, ExecOptions, SandboxEnv, Source, StreamItem};

use super::{
    error::HttpApiError,
    trace::{HttpTrace, HttpTraceCollector},
    types::ErrorCode,
};

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
    match err.downcast::<isola::Error>() {
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

        let mut stream_channels: HashMap<u32, mpsc::Sender<Bytes>> = HashMap::new();
        let mut converted_args: Vec<(Option<String>, Argument)> = Vec::new();

        for arg in &args {
            if let Some(stream_id) = extract_stream_marker(arg) {
                let (tx, rx) = mpsc::channel(64);
                stream_channels.insert(stream_id, tx);
                converted_args.push((None, Argument::cbor_stream(ReceiverStream::new(rx))));
            } else {
                match json_to_cbor_arg(arg) {
                    Ok(cbor) => converted_args.push((None, Argument::cbor(cbor))),
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
                    Argument::cbor_stream(ReceiverStream::new(rx)),
                ));
            } else {
                match json_to_cbor_arg(value) {
                    Ok(cbor) => converted_args.push((Some(name.clone()), Argument::cbor(cbor))),
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

        let (span, trace_rx, log_level) = if trace {
            let (collector, rx) = HttpTraceCollector::new();
            let s = info_span!(
                target: TRACE_TARGET_SCRIPT,
                parent: Span::current(),
                "script.exec"
            );
            if s.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector)
                .is_ok()
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
        };

        let _enter = span.enter();

        let cache_key = if trace { "trace" } else { "default" };
        let stream_result = state
            .sandbox_manager
            .exec(
                cache_key,
                source,
                function,
                converted_args,
                env,
                ExecOptions { timeout, log_level },
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

        let (trace_tx, mut trace_rx_merged) = mpsc::unbounded_channel::<HttpTrace>();

        let trace_handle = trace_rx.map(|mut rx| {
            let tx = trace_tx.clone();
            tokio::spawn(async move {
                while let Some(trace) = rx.recv().await {
                    if tx.send(trace).is_err() {
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
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Cancelled,
                                        message: "Execution cancelled".to_string(),
                                    };
                                    let _ = sender.send(make_text_message(&msg)).await;
                                    break;
                                }
                                Ok(ClientMessage::Push { stream, value }) => {
                                    if let Some(tx) = stream_channels.get(&stream)
                                        && let Ok(cbor) = json_to_cbor_arg(&value)
                                    {
                                        let _ = tx.send(cbor).await;
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
                            match cbor_to_json(&data) {
                                Ok(value) => {
                                    let msg = ServerMessage::Data { value };
                                    if sender.send(make_text_message(&msg)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
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
                            match cbor_to_json(&data) {
                                Ok(value) => {
                                    let msg = ServerMessage::Data { value };
                                    if sender.send(make_text_message(&msg)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let msg = ServerMessage::Error {
                                        code: ErrorCode::Internal,
                                        message: e.message,
                                    };
                                    let _ = sender.send(make_text_message(&msg)).await;
                                    break;
                                }
                            }
                            while let Ok(trace) = trace_rx_merged.try_recv() {
                                pending_traces.push(trace);
                            }
                            let msg = ServerMessage::Done {
                                traces: std::mem::take(&mut pending_traces),
                            };
                            let _ = sender.send(make_text_message(&msg)).await;
                            break;
                        }
                        Some(StreamItem::End(None)) | None => {
                            while let Ok(trace) = trace_rx_merged.try_recv() {
                                pending_traces.push(trace);
                            }
                            let msg = ServerMessage::Done {
                                traces: std::mem::take(&mut pending_traces),
                            };
                            let _ = sender.send(make_text_message(&msg)).await;
                            break;
                        }
                        Some(StreamItem::Error(err)) => {
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

                Some(trace) = trace_rx_merged.recv() => {
                    let msg = ServerMessage::Trace(trace);
                    if sender.send(make_text_message(&msg)).await.is_err() {
                        return;
                    }
                }
            }
        }

        if let Some(handle) = trace_handle {
            handle.abort();
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

fn json_to_cbor_arg(value: &serde_json::Value) -> Result<Bytes, HttpApiError> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| HttpApiError::invalid_request(format!("Failed to serialize arg: {e}")))?;
    cbor::json_to_cbor(&json_str)
        .map_err(|e| HttpApiError::invalid_request(format!("Failed to convert to CBOR: {e}")))
}

fn cbor_to_json(data: &Bytes) -> Result<serde_json::Value, HttpApiError> {
    let json_str = cbor::cbor_to_json(data)
        .map_err(|e| HttpApiError::internal(format!("Failed to convert CBOR to JSON: {e}")))?;
    serde_json::from_str(&json_str)
        .map_err(|e| HttpApiError::internal(format!("Failed to parse JSON: {e}")))
}
