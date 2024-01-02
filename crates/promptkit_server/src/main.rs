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

    let state = routes::AppState::new("wasm/target/promptkit_python.wasm")?;
    let app = routes::router(state);
    server::serve(app, 3000).await
}
