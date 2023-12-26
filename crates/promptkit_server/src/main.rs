use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use axum_server::Handle;
use serde_json::value::RawValue;
use server::graceful_shutdown;
use vm_manager::VmManager;

mod memory_buffer;
mod resource;
mod server;
mod vm;
mod vm_cache;
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

    let handle = Handle::new();
    tokio::spawn(graceful_shutdown(handle.clone()));

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 3000));

    Ok(axum_server::bind(addr)
        .handle(handle)
        .serve(app.into_make_service())
        .await?)
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Box<RawValue>>,
}

async fn exec(
    State(state): State<AppState>,
    Json(req): Json<ExecRequest>,
) -> Result<Response, AppError> {
    Ok(state
        .vm
        .exec(
            &req.script,
            req.method,
            req.args
                .into_iter()
                .map(|s| Box::<str>::from(s).into_string())
                .collect::<Vec<_>>(),
        )
        .await?)
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
