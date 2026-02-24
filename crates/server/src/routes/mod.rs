mod api;
mod env;
mod mcp;
mod runtime_factory;
mod sandbox_manager;
mod state;

use std::time::Duration;

use axum::{http::StatusCode, routing::get};
pub use env::{SandboxEnv, StreamItem};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
pub use runtime_factory::{Runtime, RuntimeFactory};
pub use sandbox_manager::{Argument, ExecOptions, SandboxManager, Source};
pub use state::AppState;

pub fn router(state: &AppState) -> axum::Router {
    let prometheus = match PrometheusBuilder::new().install_recorder() {
        Ok(handle) => {
            tracing::info!("Prometheus metrics recorder installed");
            Some(handle)
        }
        Err(err) => {
            tracing::warn!("Failed to install metrics recorder: {err}");
            None
        }
    };

    if let Some(handle) = prometheus.clone() {
        tokio::spawn(prometheus_upkeep(handle, Duration::from_secs(5)));
    }

    let router = axum::Router::new()
        .route("/debug/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .route("/openapi.json", get(api::openapi::openapi_json))
        .with_state(state.clone())
        .nest_service("/mcp", mcp::server(state.clone()))
        .nest("/v1", api::router(state));

    if let Some(handle) = prometheus {
        router.route(
            "/debug/metrics",
            get(move || {
                let rendered = handle.render();
                async move { rendered }
            }),
        )
    } else {
        tracing::info!("Prometheus metrics endpoint running in degraded mode");
        router.route(
            "/debug/metrics",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "metrics unavailable") }),
        )
    }
}

async fn prometheus_upkeep(handle: PrometheusHandle, duration: Duration) {
    let mut interval = tokio::time::interval(duration);
    loop {
        interval.tick().await;
        handle.run_upkeep();
    }
}
