mod api;
mod debug;
mod error;
mod state;

pub use state::State;

pub fn router(state: State) -> axum::Router {
    axum::Router::new()
        .nest("/api", api::router())
        .with_state(state)
        .merge(debug::router())
}
