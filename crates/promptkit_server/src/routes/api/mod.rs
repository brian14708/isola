use std::time::Duration;

use axum::{extract::State, routing::post, Json};
use serde_json::value::RawValue;

use crate::vm_manager::VmManager;

use super::{error::Result, state::Router};

pub fn router() -> Router {
    Router::new().route("/exec", post(exec))
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Box<RawValue>>,
}

async fn exec(State(state): State<VmManager>, Json(req): Json<ExecRequest>) -> Result {
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        state.exec(
            &req.script,
            req.method,
            req.args
                .into_iter()
                .map(|s| Box::<str>::from(s).into_string())
                .collect::<Vec<_>>(),
        ),
    )
    .await?;
    Ok(result?)
}
