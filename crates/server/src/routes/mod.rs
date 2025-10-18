mod env;
mod state;
mod vm_manager;

use std::{future::ready, time::Duration};

use axum::{http::StatusCode, response::Redirect, routing::get};
pub use env::{StreamItem, VmEnv};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
pub use state::AppState;
use tower_http::services::{ServeDir, ServeFile};
pub use vm_manager::{Argument, Source, VmManager};

pub fn router(state: &AppState) -> axum::Router {
    let prometheus = PrometheusBuilder::new().install_recorder().unwrap();
    tokio::spawn(prometheus_upkeep(
        prometheus.clone(),
        Duration::from_secs(5),
    ));

    axum::Router::new()
        .route("/", get(|| ready(Redirect::temporary("/ui"))))
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route("/debug/metrics", get(move || ready(prometheus.render())))
        .with_state(state.clone())
        .nest_service(
            "/ui",
            ServeDir::new("ui/dist").fallback(ServeFile::new("ui/dist/index.html")),
        )
}

async fn prometheus_upkeep(handle: PrometheusHandle, duration: Duration) {
    let mut interval = tokio::time::interval(duration);
    loop {
        interval.tick().await;
        handle.run_upkeep();
    }
}
