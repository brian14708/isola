use std::{path::Path, sync::Arc};

use axum::{extract::State, response::Response, routing::post, Json, Router};
use serde_json::value::RawValue;

use crate::error::ApiResult;
use crate::vm_manager::VmManager;

#[derive(Clone)]
pub struct AppState {
    vm: Arc<VmManager>,
}

pub fn router() -> anyhow::Result<(Router<AppState>, AppState)> {
    let state = AppState {
        vm: Arc::new(VmManager::new(Path::new("target/promptkit_python.wasm"))?),
    };

    Ok((Router::new().route("/exec", post(exec)), state))
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Box<RawValue>>,
}

async fn exec(State(state): State<AppState>, Json(req): Json<ExecRequest>) -> ApiResult<Response> {
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
