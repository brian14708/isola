mod error;
mod execute;
pub mod trace;
pub mod types;
mod websocket;

use axum::{Router, routing::get, routing::post};

use super::AppState;

pub fn router(state: &AppState) -> Router {
    Router::new()
        .route("/execute", post(execute::execute))
        .route("/ws/execute", get(websocket::ws_execute))
        .with_state(state.clone())
}
