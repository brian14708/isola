use std::env;

use openid::DiscoveredClient;

use crate::user::UserTokenSigner;

mod memory_buffer;
mod resource;
mod routes;
mod server;
mod user;
mod vm;
mod vm_cache;
mod vm_manager;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let client_id = env::var("OIDC_CLIENT_ID")?;
    let client_secret = env::var("OIDC_CLIENT_SECRET")?;
    let issuer_url = env::var("OIDC_ISSUER").unwrap();
    let redirect = Some(env::var("PUBLIC_HOST").unwrap() + "api/user/oidc-callback");
    let issuer = reqwest::Url::parse(&issuer_url)?;

    let host = reqwest::Url::parse(&env::var("PUBLIC_HOST").unwrap())?;
    let signer = UserTokenSigner::new("secret".as_bytes(), host.scheme() == "https");

    tracing::info!("Discovering OIDC client");
    let client = DiscoveredClient::discover(client_id, client_secret, redirect, issuer).await?;

    tracing::info!("Connecting to database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&env::var("DATABASE_URL")?)
        .await?;

    let state = routes::AppState::new("wasm/target/promptkit_python.wasm", pool, client, signer)?;
    let app = routes::router(state);
    server::serve(app, 3000).await
}
