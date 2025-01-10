mod env;
mod state;

use std::{future::ready, time::Duration};

use axum::{http::StatusCode, response::Response, routing::get};
pub use env::VmEnv;
use http::header::CONTENT_TYPE;
use metrics_exporter_prometheus::PrometheusBuilder;
pub use state::AppState;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: &AppState) -> axum::Router {
    let prometheus = PrometheusBuilder::new().install_recorder().unwrap();
    let m = prometheus.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            m.run_upkeep();
        }
    });

    axum::Router::new()
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route(
            "/debug/metrics",
            get(move || {
                let mut resp = Response::new(prometheus.render());
                resp.headers_mut().insert(CONTENT_TYPE, "".parse().unwrap());
                ready(resp)
            }),
        )
        .with_state(state.clone())
        .nest_service(
            "/ui",
            ServeDir::new("ui/build").fallback(ServeFile::new("ui/build/404.html")),
        )
}
