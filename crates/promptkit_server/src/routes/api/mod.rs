mod function;
mod user;

use std::{sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    routing::post,
    Json, Router,
};
use promptkit_executor::{ExecResult, ExecStreamItem, VmManager};
use serde_json::{json, value::RawValue};
use tokio_stream::StreamExt;

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
    let exec = vm
        .exec(
            &req.script,
            req.method,
            req.args
                .into_iter()
                .map(|s| Box::<str>::from(s).into_string())
                .collect::<Vec<_>>(),
        )
        .await?;

    match exec {
        ExecResult::Error(err) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": err.to_string() })),
        )
            .into_response()),
        ExecResult::Response(resp) => {
            Ok((StatusCode::OK, Json(RawValue::from_string(resp)?)).into_response())
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

#[derive(serde::Deserialize)]
struct SchemaRequest {
    script: String,
    method: String,
}

async fn schema(
    State(vm): State<Arc<VmManager>>,
    Json(req): Json<SchemaRequest>,
) -> crate::routes::Result {
    Ok((
        StatusCode::OK,
        Json(RawValue::from_string(
            vm.schema(&req.script, &req.method).await?,
        )?),
    )
        .into_response())
}
