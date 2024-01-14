mod function;
mod user;

use axum::Router;

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/user", user::router())
        .nest("/functions", function::router())
}
