use std::{path::Path, sync::Arc};

use axum::{
    body::Body,
    extract::FromRef,
    http::{header::CONTENT_TYPE, HeaderValue, Response, StatusCode},
    response::IntoResponse,
};
use prometheus_client::{encoding::text::encode, registry::Registry};
use promptkit_executor::VmManager;
use sqlx::PgPool;

use crate::user::UserTokenSigner;

#[derive(Clone)]
pub struct AppState {
    pub vm: Arc<VmManager>,
    pub metrics: Arc<Metrics>,
    pub pool: PgPool,
    pub openid_client: Arc<openid::Client>,
    pub token_signer: Arc<UserTokenSigner>,
}

impl AppState {
    pub fn new(
        vm_path: impl AsRef<Path>,
        pool: PgPool,
        oidc: openid::Client,
        token_signer: UserTokenSigner,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            vm: Arc::new(VmManager::new(vm_path.as_ref())?),
            metrics: Arc::new(Metrics::default()),
            pool,
            openid_client: Arc::new(oidc),
            token_signer: Arc::new(token_signer),
        })
    }
}

impl FromRef<AppState> for Arc<VmManager> {
    fn from_ref(state: &AppState) -> Self {
        state.vm.clone()
    }
}

impl FromRef<AppState> for Arc<Metrics> {
    fn from_ref(state: &AppState) -> Self {
        state.metrics.clone()
    }
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for Arc<openid::Client> {
    fn from_ref(state: &AppState) -> Self {
        state.openid_client.clone()
    }
}

impl FromRef<AppState> for Arc<UserTokenSigner> {
    fn from_ref(state: &AppState) -> Self {
        state.token_signer.clone()
    }
}

pub struct Metrics {
    pub registry: Registry,
}

impl Default for Metrics {
    fn default() -> Self {
        let registry = Registry::default();

        Self { registry }
    }
}

impl IntoResponse for &Metrics {
    fn into_response(self) -> axum::response::Response {
        let mut buffer = String::new();
        if let Err(err) = encode(&mut buffer, &self.registry) {
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
        }
    }
}
