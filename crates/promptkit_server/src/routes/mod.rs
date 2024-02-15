mod error;
mod state;

use std::{future::ready, sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
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
use tokio_stream::StreamExt;
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
    let mut tracer = req.trace.unwrap_or_default().then(MemoryTracer::new);
    let timeout = Duration::from_secs(req.timeout.unwrap_or(5));
    let args = req
        .args
        .into_iter()
        .map(|s| Box::<str>::from(s).into_string())
        .collect::<Vec<_>>();
    let exec = tokio::time::timeout(
        timeout,
        vm.exec(&req.script, req.method, args, tracer.as_mut()),
    )
    .await??;

    match exec {
        ExecResult::Error(ref err) => {
            #[derive(Serialize)]
            struct Error {
                message: String,
                trace: Option<Vec<HttpTraceEvent>>,
            }
            let e = err.to_string();
            drop(exec);
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Error {
                    message: e.to_string(),
                    trace: tracer.map(|t| t.events().map(HttpTraceEvent::from).collect::<Vec<_>>()),
                }),
            )
                .into_response())
        }
        ExecResult::Response(ref resp) => {
            let resp = resp.clone();
            drop(exec);
            Ok(if let Some(tracer) = tracer {
                (
                    StatusCode::OK,
                    Json(json!({
                        "return": RawValue::from_string(resp)?,
                        "trace": tracer.events().map(HttpTraceEvent::from).collect::<Vec<_>>(),
                    })),
                )
                    .into_response()
            } else {
                (StatusCode::OK, Json(RawValue::from_string(resp)?)).into_response()
            })
        }
        ExecResult::Stream(stream) => {
            let s = stream.map::<anyhow::Result<Event>, _>(|f| match f {
                ExecStreamItem::Data(data) => Ok(Event::default().data(data)),
                ExecStreamItem::End(end) => Ok(match end {
                    Some(data) => Event::default().data(data),
                    None => Event::default().data("[DONE]"),
                }),
                ExecStreamItem::Error(err) => Ok(Event::default()
                    .event("error")
                    .json_data(json!({
                        "message": err.to_string(),
                    }))
                    .unwrap()),
            });
            Ok(Sse::new(s)
                .keep_alive(
                    KeepAlive::new()
                        .interval(Duration::from_secs(1))
                        .text("keepalive"),
                )
                .into_response())
        }
    }
}
