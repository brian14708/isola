use axum::Router;

mod api;
mod debug;

pub fn router() -> anyhow::Result<Router> {
    let (route, state) = api::router()?;
    Ok(Router::new()
        .nest("/api", route)
        .with_state(state)
        .merge(debug::router()))
}
