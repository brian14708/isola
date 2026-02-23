mod error;
mod execute;
pub mod openapi;
pub mod trace;
mod trace_context;
pub mod types;
mod websocket;

use axum::{
    Router,
    routing::{get, post},
};
use tower_http::trace::TraceLayer;

use super::AppState;

pub fn router(state: &AppState) -> Router {
    let traced_http_routes = Router::new()
        .route("/execute", post(execute::execute_sync))
        .route("/execute/stream", post(execute::execute_stream))
        .layer(TraceLayer::new_for_http().make_span_with(trace_context::make_server_span));

    Router::new()
        .merge(traced_http_routes)
        .route("/execute/ws", get(websocket::ws_execute))
        .with_state(state.clone())
}
