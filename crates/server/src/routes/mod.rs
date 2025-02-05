mod env;
mod state;

use std::{future::ready, time::Duration};

use axum::{http::StatusCode, routing::get};
pub use env::VmEnv;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
pub use state::AppState;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: &AppState) -> axum::Router {
    let prometheus = PrometheusBuilder::new().install_recorder().unwrap();
    tokio::spawn(prometheus_upkeep(
        prometheus.clone(),
        Duration::from_secs(5),
    ));

    axum::Router::new()
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route("/debug/metrics", get(move || ready(prometheus.render())))
        .with_state(state.clone())
        .nest_service(
            "/ui",
            ServeDir::new("ui/build").fallback(ServeFile::new("ui/build/404.html")),
        )
}

async fn prometheus_upkeep(handle: PrometheusHandle, duration: Duration) {
    let mut interval = tokio::time::interval(duration);
    loop {
        interval.tick().await;
        handle.run_upkeep();
    }
}
