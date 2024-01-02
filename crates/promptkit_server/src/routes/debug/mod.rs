use std::{future::ready, sync::Arc};

use axum::{
    body::Body,
    extract::State,
    http::{header::CONTENT_TYPE, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use prometheus_client::encoding::text::encode;

use super::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route(
            "/debug/metrics",
            get(|State(state): State<Arc<AppState>>| {
                let mut buffer = String::new();
                ready(
                    if let Err(err) = encode(&mut buffer, &state.metrics.registry) {
                        tracing::error!("failed to encode metrics: {}", err);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    } else {
                        let mut resp = Response::new(Body::from(buffer));
                        resp.headers_mut().insert(
                            CONTENT_TYPE,
                            HeaderValue::from_static(
                                "application/openmetrics-text; version=1.0.0; charset=utf-8",
                            ),
                        );
                        resp
                    },
                )
            }),
        )
}
