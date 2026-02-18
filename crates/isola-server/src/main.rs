use std::env::args;

use anyhow::anyhow;
use utils::otel::init_tracing;

use crate::routes::{VmEnv, VmManager};

mod routes;
mod server;
mod utils;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
    let task = args().nth(1);
    match task.as_deref() {
        Some("build") => {
            _ = VmManager::<VmEnv>::new("target/promptkit_python.wasm").await?;
            Ok(())
        }
        None | Some("serve") => {
            let _provider = init_tracing()?;
            let state = routes::AppState::new("target/promptkit_python.wasm").await?;
            let app = routes::router(&state);

            server::serve(
                app,
                std::env::var("PORT")
                    .ok()
                    .and_then(|p| p.parse::<u16>().ok())
                    .unwrap_or(3000),
            )
            .await
        }
        _ => Err(anyhow!("unknown task")),
    }
}
