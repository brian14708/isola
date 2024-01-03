mod api;
mod error;
mod state;

use std::{future::ready, sync::Arc};

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get};
pub use error::Result;
pub use state::{AppState, Metrics};

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .nest("/api", api::router())
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route(
            "/debug/metrics",
            get(|State(metrics): State<Arc<Metrics>>| ready(metrics.into_response())),
        )
        .with_state(state)
}
