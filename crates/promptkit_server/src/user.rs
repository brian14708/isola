use std::ops::{Add, Sub};

use axum_extra::extract::{
    cookie::{Cookie, Expiration},
    CookieJar,
};
use jsonwebtoken::{decode, DecodingKey, EncodingKey, Validation};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

#[derive(Serialize, Deserialize)]
pub struct UserToken {}

#[derive(Serialize, Deserialize)]
struct UserTokenClaims {
    #[serde(flatten)]
    base: UserBaseToken,

    #[serde(flatten)]
    token: UserToken,
}

#[derive(Serialize, Deserialize)]
struct UserBaseToken {
    sub: String,
    exp: i64,
    iat: i64,
    iss: String,
    nbf: i64,
}

pub struct UserTokenSigner {
    secure: bool,
    encode_key: EncodingKey,
    decode_key: DecodingKey,
}

pub enum VerifyStatus {
    Ok(String, UserToken),
    Refresh(String),
    Invalid,
}

impl UserTokenSigner {
    pub fn new(key: &[u8], secure: bool) -> Self {
        let encode_key = EncodingKey::from_secret(key);
        let decode_key = DecodingKey::from_secret(key);

        Self {
            secure,
            encode_key,
            decode_key,
        }
    }

    pub fn sign(&self, id: String, token: UserToken) -> anyhow::Result<Cookie> {
        const EXP: i64 = 60 * 15;
        let now = OffsetDateTime::now_utc();
        let expire = now.add(Duration::seconds(EXP));

        let token = UserTokenClaims {
            base: UserBaseToken {
                sub: id,
                exp: expire.unix_timestamp(),
                iat: now.unix_timestamp(),
                iss: "pkt".to_owned(),
                nbf: now.sub(Duration::minutes(1)).unix_timestamp(),
            },
            token,
        };
        let m = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &token, &self.encode_key)?;
        Ok(Cookie::build(("pkt-token", m))
            .path("/")
            .expires(Expiration::from(expire))
            .http_only(true)
            .secure(self.secure)
            .into())
    }

    pub fn sign_refresh(&self, id: String) -> anyhow::Result<Cookie> {
        const EXP: i64 = 60 * 60 * 24 * 7;
        let now = OffsetDateTime::now_utc();
        let expire = now.add(Duration::seconds(EXP));

        let token = UserBaseToken {
            sub: id,
            exp: expire.unix_timestamp(),
            iat: now.unix_timestamp(),
            iss: "pkt".to_string(),
            nbf: now.sub(Duration::minutes(1)).unix_timestamp(),
        };

        let m = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &token, &self.encode_key)?;
        Ok(Cookie::build(("pkt-refresh-token", m))
            .path("/")
            .expires(Expiration::from(expire))
            .http_only(true)
            .secure(self.secure)
            .into())
    }

    pub fn verify(&self, cookie: &CookieJar) -> VerifyStatus {
        let token = cookie.get("pkt-token").map(|f| f.value());
        let refresh = cookie.get("pkt-refresh-token").map(|f| f.value());

        if let Some(token) = token {
            if let Ok(token) =
                decode::<UserTokenClaims>(token, &self.decode_key, &Validation::default())
            {
                return VerifyStatus::Ok(token.claims.base.sub, token.claims.token);
            }
        }

        if let Some(refresh) = refresh {
            if let Ok(token) =
                decode::<UserBaseToken>(refresh, &self.decode_key, &Validation::default())
            {
                return VerifyStatus::Refresh(token.claims.sub);
            }
        }

        VerifyStatus::Invalid
    }
}
