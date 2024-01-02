use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde_json::value::RawValue;

use super::{error::Result, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/exec", post(exec))
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Box<RawValue>>,
}

async fn exec(State(state): State<Arc<AppState>>, Json(req): Json<ExecRequest>) -> Result {
    let result = state
        .vm
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
