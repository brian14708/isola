mod error;
mod state;

use std::{future::ready, sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json,
};
pub use error::Result;
use promptkit_executor::{
    trace::{MemoryTracer, TraceEvent, TraceLogLevel},
    ExecResult, ExecStreamItem, VmManager,
};
use serde::Serialize;
use serde_json::{json, value::RawValue};
pub use state::{AppState, Metrics};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: AppState) -> axum::Router {
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
    args: Vec<Box<RawValue>>,
    timeout: Option<u64>,
    trace: Option<bool>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum HttpTraceEvent {
    Log {
        level: &'static str,
        content: String,
        timestamp: f32,
    },
}

impl From<TraceEvent> for HttpTraceEvent {
    fn from(value: TraceEvent) -> Self {
        match value {
            TraceEvent::Log {
                content,
                level,
                timestamp,
            } => Self::Log {
                content,
                level: match level {
                    TraceLogLevel::Stdout => "info",
                    TraceLogLevel::Stderr => "error",
                },
                timestamp: timestamp.as_f64() as f32,
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
        .map(|(a, b)| (Some(a), Some(b)))
        .unwrap_or_default();
    let timeout = Duration::from_secs(req.timeout.unwrap_or(5));
    let args = req
        .args
        .into_iter()
        .map(|s| Box::<str>::from(s).into_string())
        .collect::<Vec<_>>();
    let exec =
        tokio::time::timeout(timeout, vm.exec(&req.script, req.method, args, tracer)).await??;

    let resp = match exec {
        ExecResult::Error(err) => {
            #[derive(Serialize)]
            struct Error {
                message: String,
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Error {
                    message: err.to_string(),
                }),
            )
                .into_response()
        }
        ExecResult::Stream(mut stream) => {
            if let Some(tracer) = trace_events {
                let s = stream.map(exec_to_event);
                let tracer = ReceiverStream::new(tracer).map::<anyhow::Result<Event>, _>(|e| {
                    Ok(Event::default()
                        .event("trace")
                        .json_data(HttpTraceEvent::from(e))?)
                });
                stream_response(s.merge(tracer))
            } else {
                let first = match stream.next().await {
                    Some(ExecStreamItem::End(end)) => {
                        return Ok(match end {
                            Some(data) => Response::builder()
                                .status(StatusCode::OK)
                                .header(
                                    CONTENT_TYPE,
                                    HeaderValue::from_static(mime::APPLICATION_JSON.as_ref()),
                                )
                                .body(data.into())?,
                            None => StatusCode::NO_CONTENT.into_response(),
                        });
                    }
                    Some(first) => first,
                    None => return Ok(StatusCode::NO_CONTENT.into_response()),
                };

                stream_response(tokio_stream::once(first).chain(stream).map(exec_to_event))
            }
        }
    };
    Ok(resp)
}

fn exec_to_event(e: ExecStreamItem) -> anyhow::Result<Event> {
    Ok(match e {
        ExecStreamItem::Data(data) => Event::default().data(data),
        ExecStreamItem::End(end) => match end {
            Some(data) => Event::default().data(data),
            None => Event::default().data("[DONE]"),
        },
        ExecStreamItem::Error(err) => Event::default().event("error").json_data(json!({
            "message": err.to_string(),
        }))?,
    })
}

fn stream_response<S>(stream: S) -> Response
where
    S: Stream<Item = anyhow::Result<Event>> + Send + 'static,
{
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(1))
                .text("keepalive"),
        )
        .into_response()
}
