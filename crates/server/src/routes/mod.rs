mod env;
mod state;

use std::{future::ready, sync::Arc};

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get};
pub use env::VmEnv;
pub use state::{AppState, Metrics};
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: &AppState) -> axum::Router {
    axum::Router::new()
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route(
            "/debug/metrics",
            get(|State(metrics): State<Arc<Metrics>>| ready(metrics.into_response())),
        )
        .with_state(state.clone())
        .nest_service(
            "/ui",
            ServeDir::new("ui/build").fallback(ServeFile::new("ui/build/404.html")),
        )
}
