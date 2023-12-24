use std::{path::Path, sync::Arc};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use serde_json::Value;
use vm_manager::{ExecResult, VmManager};

mod vm_manager;

#[derive(Clone)]
struct AppState {
    vm: Arc<VmManager>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let state = AppState {
        vm: Arc::new(VmManager::new(Path::new("target/promptkit_python.wasm"))?),
    };

    let app = Router::new()
        .route(
            "/debug/healthz",
            get(|| async { (StatusCode::NO_CONTENT,) }),
        )
        .route("/exec", post(exec))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Value>,
}

async fn exec(
    State(state): State<AppState>,
    Json(req): Json<ExecRequest>,
) -> Result<Json<ExecResult>, AppError> {
    let s = state
        .vm
        .exec(
            &req.script,
            &req.method,
            &req.args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .await?;
    Ok(Json(s))
}

struct AppError(anyhow::Error);

#[derive(serde::Serialize)]
struct ErrorResponse {
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                message: self.0.root_cause().to_string(),
            }),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
