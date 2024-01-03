use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
    routing::get,
    Router,
};
use openid::{Options, Token};
use serde::Deserialize;
use serde_json::json;
use sqlx::{types::Json, PgPool};

use crate::routes::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", get(login))
        .route("/oidc-callback", get(callback))
}

async fn login(State(oidc): State<Arc<openid::Client>>) -> impl IntoResponse {
    let auth_url = oidc.auth_url(&Options {
        scope: Some("openid email".into()),
        ..Default::default()
    });

    Redirect::to(auth_url.as_str())
}

#[derive(Deserialize, Debug)]
pub struct LoginQuery {
    pub code: String,
    pub state: Option<String>,
}

async fn callback(
    State(oidc): State<Arc<openid::Client>>,
    State(pool): State<PgPool>,
    query: Query<LoginQuery>,
) -> crate::routes::Result<impl IntoResponse> {
    let mut token: Token = oidc.request_token(&query.code).await?.into();

    if let Some(id_token) = token.id_token.as_mut() {
        oidc.decode_token(id_token)?;
        oidc.validate_token(id_token, None, None)?;
    } else {
        return Err(anyhow::anyhow!("no id token").into());
    }

    let userinfo = oidc.request_userinfo(&token).await?;

    let name = userinfo.name.unwrap_or_default();
    let email = userinfo.email.unwrap();

    sqlx::query(
        "INSERT INTO promptkit.users (name, email, profile) VALUES ($1, $2, $3) ON CONFLICT (email) DO UPDATE SET (name, profile) = ($1, $3)",
    )
    .bind(name)
    .bind(email)
    .bind(Json(json!({
        "avatar_url": userinfo.picture.map(|f| f.to_string())
    })))
    .execute(&pool)
    .await?;

    if let Some(state) = &query.state {
        Ok(Redirect::to(state))
    } else {
        Ok(Redirect::to("/"))
    }
}
