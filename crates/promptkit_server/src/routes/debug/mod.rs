use axum::{http::StatusCode, routing::get, Router};

pub fn router() -> Router {
    Router::new().route(
        "/debug/healthz",
        get(|| async { (StatusCode::NO_CONTENT,) }),
    )
}
