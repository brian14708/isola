use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header::SET_COOKIE, request::Parts, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::CookieJar;
use sqlx::{types::Uuid, PgPool};

use crate::user::{UserToken, UserTokenSigner, VerifyStatus};

#[derive(Clone)]
pub struct AuthSession {
    pub user_id: Uuid,
}

pub async fn auth(
    State(signer): State<Arc<UserTokenSigner>>,
    State(pool): State<PgPool>,
    jar: CookieJar,
    mut req: Request,
    next: Next,
) -> Response {
    match signer.verify(&jar) {
        VerifyStatus::Ok(id, _) => {
            req.extensions_mut().insert(AuthSession {
                user_id: Uuid::parse_str(&id).unwrap(),
            });
        }
        VerifyStatus::Refresh(id) => {
            if let Ok(uid) = Uuid::parse_str(&id) {
                if let Ok(_r) = sqlx::query!("SELECT name FROM promptkit.users WHERE id = $1", uid)
                    .fetch_one(&pool)
                    .await
                {
                    let token = signer.sign(id, UserToken {});
                    if let Ok(token) = token {
                        req.extensions_mut().insert(AuthSession { user_id: uid });
                        let mut resp = next.run(req).await;
                        resp.headers_mut().insert(
                            SET_COOKIE,
                            HeaderValue::try_from(token.to_string()).unwrap(),
                        );
                        return resp;
                    }
                }
            }
        }
        VerifyStatus::Invalid => {}
    }
    next.run(req).await
}

#[async_trait::async_trait]
impl<S> FromRequestParts<S> for AuthSession
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(a) = parts.extensions.get::<AuthSession>() {
            Ok(a.clone())
        } else {
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}
