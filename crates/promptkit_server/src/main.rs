use std::env;

use openid::DiscoveredClient;

mod memory_buffer;
mod resource;
mod routes;
mod server;
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
    let redirect = Some("http://localhost:3000/api/user/oidc-callback".to_owned());
    let issuer = reqwest::Url::parse(&issuer_url)?;

    tracing::info!("Discovering OIDC client");
    let client = DiscoveredClient::discover(client_id, client_secret, redirect, issuer).await?;

    tracing::info!("Connecting to database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&env::var("DATABASE_URL")?)
        .await?;

    let state = routes::AppState::new("wasm/target/promptkit_python.wasm", pool, client)?;
    let app = routes::router(state);
    server::serve(app, 3000).await
}
