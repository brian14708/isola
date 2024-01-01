use std::future::ready;

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use prometheus::{Encoder, TextEncoder, TEXT_FORMAT};

pub fn router() -> Router {
    Router::new()
        .route("/debug/healthz", get(|| ready(StatusCode::NO_CONTENT)))
        .route(
            "/debug/metrics",
            get(|| {
                let mut buffer = vec![];
                let encoder = TextEncoder::new();
                ready(
                    if encoder.encode(&prometheus::gather(), &mut buffer).is_err() {
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    } else {
                        let mut resp = Response::new(Body::from(buffer));
                        resp.headers_mut()
                            .insert("Content-Type", HeaderValue::from_static(TEXT_FORMAT));
                        resp
                    },
                )
            }),
        )
}
