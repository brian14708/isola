mod error;
mod state;

use std::{borrow::Cow, future::ready, pin::Pin, sync::Arc, time::Duration};

use axum::{
    extract::{
        ws::{
            close_code::{ABNORMAL, NORMAL, UNSUPPORTED},
            CloseFrame, Message,
        },
        State, WebSocketUpgrade,
    },
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{sse::Event, IntoResponse, Response},
    routing::{get, post},
    Json,
};
pub use error::Result;
use futures_util::{FutureExt, Sink, SinkExt, StreamExt};
use promptkit_executor::{
    trace::{BoxedTracer, MemoryTracer, TraceEvent, TraceEventKind},
    ExecStreamItem, VmManager,
};
use serde_json::{json, value::RawValue};
pub use state::{AppState, Metrics};
use tokio::select;
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tower_http::services::{ServeDir, ServeFile};

use crate::utils::stream::StreamResponse;

pub fn router(state: &AppState) -> axum::Router {
    axum::Router::new()
        .route("/v1/code/exec", post(exec).get(exec_ws))
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route(
            "/debug/metrics",
            get(|State(metrics): State<Arc<Metrics>>| ready(metrics.into_response())),
        )
        .with_state(state.clone())
        .nest_service(
            "/ui",
            ServeDir::new("ui/dist").fallback(ServeFile::new("ui/dist/index.html")),
        )
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Option<Vec<Box<RawValue>>>,
    timeout: Option<u64>,
    trace: Option<bool>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct HttpTraceEvent {
    id: i16,
    group: &'static str,
    timestamp: f32,
    #[serde(flatten)]
    kind: HttpTraceEventKind,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum HttpTraceEventKind {
    Log {
        content: String,
    },
    Event {
        kind: Cow<'static, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<i16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Box<RawValue>>,
    },
    SpanBegin {
        kind: Cow<'static, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<i16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Box<RawValue>>,
    },
    SpanEnd {
        parent_id: i16,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Box<RawValue>>,
    },
}

impl From<TraceEvent> for HttpTraceEvent {
    fn from(value: TraceEvent) -> Self {
        Self {
            id: value.id,
            group: value.group,
            #[allow(clippy::cast_possible_truncation)]
            timestamp: value.timestamp.as_f64() as f32,
            kind: match value.kind {
                TraceEventKind::Log { content } => HttpTraceEventKind::Log { content },
                TraceEventKind::Event {
                    kind,
                    parent_id,
                    data,
                } => HttpTraceEventKind::Event {
                    kind,
                    parent_id,
                    data,
                },
                TraceEventKind::SpanBegin {
                    kind,
                    parent_id,
                    data,
                } => HttpTraceEventKind::SpanBegin {
                    kind,
                    parent_id,
                    data,
                },
                TraceEventKind::SpanEnd { parent_id, data } => {
                    HttpTraceEventKind::SpanEnd { parent_id, data }
                }
            },
        }
    }
}

async fn exec(
    State(vm): State<Arc<VmManager>>,
    Json(req): Json<ExecRequest>,
) -> crate::routes::Result {
    let (tracer, trace_events) = req
        .trace
        .unwrap_or_default()
        .then(MemoryTracer::new)
        .map(|(a, b)| -> (Option<BoxedTracer>, _) { (Some(a), Some(b)) })
        .unwrap_or_default();

    let timeout = Duration::from_secs(req.timeout.unwrap_or(5));
    let args = req.args.unwrap_or_default().into_iter().collect::<Vec<_>>();
    let mut stream = Box::pin(
        tokio_stream::StreamExt::timeout(
            tokio::time::timeout(timeout, vm.exec(&req.script, req.method, args, tracer)).await??,
            timeout,
        )
        .map(|e| match e {
            Ok(data) => data,
            Err(_) => ExecStreamItem::Error(anyhow::anyhow!("timeout")),
        }),
    );

    let resp = if let Some(tracer) = trace_events {
        let s = stream.map(exec_to_event);
        let tracer = ReceiverStream::new(tracer).map::<anyhow::Result<Event>, _>(|e| {
            Ok(Event::default()
                .event("trace")
                .json_data(HttpTraceEvent::from(e))?)
        });
        StreamResponse(tokio_stream::StreamExt::merge(s, tracer)).into_response()
    } else {
        let first = match stream.next().await {
            Some(ExecStreamItem::End(end)) => {
                while stream.next().await.is_some() {}
                return Ok(match end {
                    Some(data) => Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                        .body(data.into())?,
                    None => StatusCode::NO_CONTENT.into_response(),
                });
            }
            Some(ExecStreamItem::Data(first)) => first,
            Some(ExecStreamItem::Error(err)) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "message": err.to_string(),
                    })),
                )
                    .into_response())
            }
            None => return Ok(StatusCode::NO_CONTENT.into_response()),
        };

        StreamResponse(
            tokio_stream::once(ExecStreamItem::Data(first))
                .chain(stream)
                .map(exec_to_event),
        )
        .into_response()
    };
    Ok(resp)
}

fn exec_to_event(e: ExecStreamItem) -> anyhow::Result<Event> {
    match e {
        ExecStreamItem::Data(data) | ExecStreamItem::End(Some(data)) => {
            Ok(Event::default().data(data))
        }
        ExecStreamItem::End(None) => Ok(Event::default().data("[DONE]")),
        ExecStreamItem::Error(err) => Err(err),
    }
}

async fn exec_ws(ws: WebSocketUpgrade, State(vm): State<Arc<VmManager>>) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        let r: Option<ExecRequest> = if let Some(Ok(first)) = socket.recv().await {
            first
                .to_text()
                .ok()
                .and_then(|s| serde_json::from_str(s).ok())
        } else {
            None
        };
        let Some(req) = r else {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: UNSUPPORTED,
                    reason: "invalid request".into(),
                })))
                .await;
            return;
        };
        let (mut send, mut recv) = socket.split();
        if let Err(e) = handle_socket(&mut send, &mut recv, req, vm).await {
            let _ = send
                .send(Message::Close(Some(CloseFrame {
                    code: ABNORMAL,
                    reason: e.to_string().into(),
                })))
                .await;
        } else {
            let _ = send
                .send(Message::Close(Some(CloseFrame {
                    code: NORMAL,
                    reason: "".into(),
                })))
                .await;
        }
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum WsMessage {
    Trace(HttpTraceEvent),
    Data(Box<RawValue>),
}

#[allow(clippy::too_many_lines)]
async fn handle_socket(
    send: &mut (impl Sink<Message> + Unpin),
    recv: &mut (impl Stream<Item = core::result::Result<Message, axum::Error>> + Unpin),
    req: ExecRequest,
    vm: Arc<VmManager>,
) -> anyhow::Result<()> {
    let (tracer, mut trace_events) = req
        .trace
        .unwrap_or_default()
        .then(MemoryTracer::new)
        .map(|(a, b)| -> (Option<BoxedTracer>, _) { (Some(a), Some(b)) })
        .unwrap_or_default();

    let timeout = Duration::from_secs(req.timeout.unwrap_or(5));
    let (tx, rx) = tokio::sync::mpsc::channel::<Box<RawValue>>(1);
    let rx: Pin<Box<dyn Stream<Item = _> + Send>> = Box::pin(ReceiverStream::new(rx));

    let mut stream = Box::pin(
        tokio_stream::StreamExt::timeout(
            tokio::time::timeout(timeout, vm.exec(&req.script, req.method, [rx], tracer)).await??,
            timeout,
        )
        .map(|e| match e {
            Ok(data) => data,
            Err(_) => ExecStreamItem::Error(anyhow::anyhow!("timeout")),
        }),
    );

    let mut input = Box::pin(
        async move {
            loop {
                let msg = recv.next().await;
                let msg = if let Some(Ok(e)) = msg {
                    let e = e.into_text();
                    if let Ok(e) = e {
                        if e == "[DONE]" {
                            break;
                        }
                        if let Ok(e) = RawValue::from_string(e) {
                            e
                        } else {
                            return Err(anyhow::anyhow!("invalid message"));
                        }
                    } else {
                        return Err(anyhow::anyhow!("invalid message"));
                    }
                } else {
                    return Err(anyhow::anyhow!("invalid message"));
                };

                tx.send(msg).await?;
            }
            Ok(())
        }
        .fuse(),
    );

    if let Some(te) = trace_events.as_mut() {
        loop {
            let e = select! {
                Err(e) = &mut input => return Err(e),
                Some(e) = stream.next() => e,
                Some(e) = te.recv() => {
                    let _ = send
                        .send(Message::Text(
                            serde_json::to_string(&WsMessage::Trace(HttpTraceEvent::from(e)))
                                .unwrap(),
                        ))
                        .await;
                    continue;
                },
                else => return Ok(()),
            };
            match e {
                ExecStreamItem::Data(d) => {
                    let _ = send
                        .send(Message::Text(
                            serde_json::to_string(&WsMessage::Data(
                                RawValue::from_string(d).unwrap(),
                            ))
                            .unwrap(),
                        ))
                        .await;
                }
                ExecStreamItem::End(d) => {
                    if let Some(d) = d {
                        let _ = send
                            .send(Message::Text(
                                serde_json::to_string(&WsMessage::Data(
                                    RawValue::from_string(d).unwrap(),
                                ))
                                .unwrap(),
                            ))
                            .await;
                    }
                    return Ok(());
                }
                ExecStreamItem::Error(err) => {
                    return Err(err);
                }
            }
        }
    } else {
        loop {
            let e = select! {
                Err(e) = &mut input => return Err(e),
                Some(e) = stream.next() => e,
                else => return Ok(()),
            };
            match e {
                ExecStreamItem::Data(d) => {
                    let _ = send.send(Message::Text(d)).await;
                }
                ExecStreamItem::End(d) => {
                    if let Some(d) = d {
                        let _ = send.send(Message::Text(d)).await;
                    }
                    return Ok(());
                }
                ExecStreamItem::Error(err) => {
                    return Err(err);
                }
            }
        }
    }
}
