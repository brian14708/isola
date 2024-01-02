mod api;
mod debug;
mod error;
mod state;

use std::sync::Arc;

pub use state::AppState;

pub fn router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .nest("/api", api::router())
        .merge(debug::router())
        .with_state(state)
}
