mod user;

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde_json::value::RawValue;

use crate::vm_manager::VmManager;

use super::{AppState, Result};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/exec", post(exec))
        .nest("/user", user::router())
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Box<RawValue>>,
}

async fn exec(State(vm): State<Arc<VmManager>>, Json(req): Json<ExecRequest>) -> Result {
    let result = vm
        .exec(
            &req.script,
            req.method,
            req.args
                .into_iter()
                .map(|s| Box::<str>::from(s).into_string())
                .collect::<Vec<_>>(),
        )
        .await?;
    Ok(result)
}
