use std::env::args;

use anyhow::anyhow;
use utils::otel::init_tracing;

use crate::routes::{SandboxEnv, SandboxManager};

mod request;
mod routes;
mod server;
mod utils;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
const WASM_PATH: &str = "target/python3.wasm";

fn main() -> anyhow::Result<()> {
    if let Err(e) = rlimit::increase_nofile_limit(u64::MAX) {
        tracing::warn!("Failed to raise ulimit: {e}");
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let _provider = init_tracing()?;
    let task = args().nth(1);
    match task.as_deref() {
        Some("build") => {
            tracing::info!(
                task = "build",
                wasm_path = WASM_PATH,
                "Building sandbox template"
            );
            _ = SandboxManager::<SandboxEnv>::new(WASM_PATH).await?;
            Ok(())
        }
        None | Some("serve") => {
            let _provider = init_tracing()?;
            let port = std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(3000);
            tracing::info!(
                task = "serve",
                wasm_path = WASM_PATH,
                port,
                "Starting isola-server"
            );
            let state = routes::AppState::new(WASM_PATH).await?;
            let app = routes::router(&state);

            server::serve(app, port).await
        }
        _ => Err(anyhow!("unknown task")),
    }
}
