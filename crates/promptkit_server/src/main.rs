use std::{path::Path, sync::Arc};

use axum::{
    extract::State,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use script_manager::{ScriptRunner};
use serde_json::{json, Value};


mod script_manager;

#[derive(Clone)]
struct AppState {
    r: Arc<ScriptRunner>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let r = Arc::new(ScriptRunner::new(Path::new(
        "target/promptkit_python.wasm",
    ))?);
    let state = AppState { r: r.clone() };

    tracing_subscriber::fmt::init();
    let app = Router::new()
        .route("/", get(root))
        .route("/exec", post(exec))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

async fn root() -> Json<Value> {
    Json(json!("A"))
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Value>,
}

async fn exec(State(state): State<AppState>, Json(req): Json<ExecRequest>) -> Response {
    let s = state
        .r
        .create_script()
        .await
        .unwrap()
        .run(
            &req.script,
            req.method,
            req.args.iter().map(|s| s.to_string()).collect(),
        )
        .await
        .unwrap();
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(s.into())
        .unwrap()
}
