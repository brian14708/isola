mod function;
mod user;

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde_json::value::RawValue;

use crate::vm_manager::VmManager;

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/user", user::router())
        .nest("/functions", function::router())
        .route("/exec", post(exec))
        .route("/schema", post(schema))
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    script: String,
    method: String,
    args: Vec<Box<RawValue>>,
}

async fn exec(
    State(vm): State<Arc<VmManager>>,
    Json(req): Json<ExecRequest>,
) -> crate::routes::Result {
    Ok(vm
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

#[derive(serde::Deserialize)]
struct SchemaRequest {
    script: String,
    method: String,
}

async fn schema(
    State(vm): State<Arc<VmManager>>,
    Json(req): Json<SchemaRequest>,
) -> crate::routes::Result {
    Ok(vm.schema(&req.script, &req.method).await?)
}
