use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{
        header::{LOCATION, SET_COOKIE},
        Response, StatusCode,
    },
    response::{IntoResponse, Redirect},
    routing::get,
    Router,
};
use openid::{Options, Token};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::{
    routes::{auth::AuthSession, AppState},
    user::UserTokenSigner,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(user_info))
        .route("/login", get(login))
        .route("/oidc-callback", get(callback))
}

async fn user_info(State(pool): State<PgPool>, auth: AuthSession) -> crate::routes::Result {
    let u = sqlx::query!(
        "SELECT name, profile FROM promptkit.users WHERE id = $1",
        auth.user_id
    )
    .fetch_one(&pool)
    .await?;
    Ok(axum::Json(json!({
        "id": auth.user_id.to_string(),
        "name": u.name,
        "profile": u.profile
    }))
    .into_response())
}

#[derive(Deserialize)]
struct LoginQuery {
    redirect: Option<String>,
}

async fn login(
    State(oidc): State<Arc<openid::Client>>,
    Query(query): Query<LoginQuery>,
) -> impl IntoResponse {
    let auth_url = oidc.auth_url(&Options {
        scope: Some("openid email".into()),
        state: query.redirect,
        ..Default::default()
    });

    Redirect::to(auth_url.as_str())
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: Option<String>,
}

async fn callback(
    State(oidc): State<Arc<openid::Client>>,
    State(pool): State<PgPool>,
    State(signer): State<Arc<UserTokenSigner>>,
    query: Query<CallbackQuery>,
) -> crate::routes::Result {
    let mut token: Token = oidc.request_token(&query.code).await?.into();

    if let Some(id_token) = token.id_token.as_mut() {
        oidc.decode_token(id_token)?;
        oidc.validate_token(id_token, None, None)?;
    } else {
        return Err(anyhow::anyhow!("no id token").into());
    }

    let userinfo = oidc.request_userinfo(&token).await?;

    let name = userinfo.name.unwrap_or_default();
    let email = if let Some(email) = userinfo.email {
        email
    } else {
        return Err(anyhow::anyhow!("no email").into());
    };

    let user =  sqlx::query!(
        "INSERT INTO promptkit.users (name, email, profile) VALUES ($1, $2, $3) ON CONFLICT (email) DO UPDATE SET (name, profile) = ($1, $3) RETURNING id",
        name,
        email,
        json!({
            "avatar_url": userinfo.picture.map(|f| f.to_string())
        }),
    )
    .fetch_one(&pool)
    .await?;

    let refresh = signer.sign_refresh(user.id.to_string())?;
    Ok(Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(LOCATION, query.0.state.unwrap_or_else(|| "/".to_string()))
        .header(SET_COOKIE, refresh.to_string())
        .body(Body::empty())?)
}
