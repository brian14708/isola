mod api;
mod env;
mod mcp;
mod state;
mod vm_manager;

use std::time::Duration;

use axum::{http::StatusCode, routing::get};
pub use env::{StreamItem, VmEnv};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
pub use state::AppState;
pub use vm_manager::{Argument, Source, VmManager};

pub fn router(state: &AppState) -> axum::Router {
    let prometheus = PrometheusBuilder::new().install_recorder().unwrap();
    tokio::spawn(prometheus_upkeep(
        prometheus.clone(),
        Duration::from_secs(5),
    ));

    axum::Router::new()
        .route("/debug/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/debug/metrics",
            get(move || {
                let rendered = prometheus.render();
                async move { rendered }
            }),
        )
        .with_state(state.clone())
        .nest_service("/mcp", mcp::server(state.clone()))
        .nest("/api/v1", api::router(state))
}

async fn prometheus_upkeep(handle: PrometheusHandle, duration: Duration) {
    let mut interval = tokio::time::interval(duration);
    loop {
        interval.tick().await;
        handle.run_upkeep();
    }
}
