mod api;
mod debug;
mod error;
mod state;

pub use error::Result;
pub use state::{AppState, Metrics};

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .nest("/api", api::router())
        .merge(debug::router())
        .with_state(state)
}
