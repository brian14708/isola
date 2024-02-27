mod error;
mod state;

use std::{borrow::Cow, future::ready, sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{sse::Event, IntoResponse, Response},
    routing::{get, post},
    Json,
};
use cbor4ii::core::utils::{BufWriter, SliceReader};
pub use error::Result;
use futures_util::StreamExt;
use promptkit_executor::{
    trace::{BoxedTracer, MemoryTracer, TraceEvent, TraceEventKind},
    ExecArgument, ExecStreamItem, VmManager,
};
use serde::Serialize;
use serde_json::{json, value::RawValue};
pub use state::{AppState, Metrics};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::services::{ServeDir, ServeFile};

use crate::utils::stream::StreamResponse;

pub fn router(state: &AppState) -> axum::Router {
    axum::Router::new()
        .route("/v1/code/exec", post(exec))
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
    let args = req
        .args
        .unwrap_or_default()
        .into_iter()
        .map(|v| {
            let v: Box<str> = v.into();
            let mut o = BufWriter::new(vec![]);
            let mut s = cbor4ii::serde::Serializer::new(&mut o);
            serde_transcode::Transcoder::new(&mut serde_json::Deserializer::from_str(&v))
                .serialize(&mut s)
                .unwrap();
            ExecArgument::Cbor(o.into_inner())
        })
        .collect::<Vec<_>>();
    let mut stream = Box::pin(
        tokio_stream::StreamExt::timeout(
            tokio::time::timeout(timeout, vm.exec(&req.script, &req.method, args, tracer))
                .await??,
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
                        .body(cbor_to_json(&data)?.into())?,
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
            Ok(Event::default().data(cbor_to_json(&data)?))
        }
        ExecStreamItem::End(None) => Ok(Event::default().data("[DONE]")),
        ExecStreamItem::Error(err) => Err(err),
    }
}

fn cbor_to_json(s: &[u8]) -> anyhow::Result<String> {
    let mut o = vec![];
    let mut s = SliceReader::new(s);
    serde_transcode::Transcoder::new(&mut cbor4ii::serde::Deserializer::new(&mut s))
        .serialize(&mut serde_json::Serializer::new(&mut o))?;
    Ok(String::from_utf8(o)?)
}
