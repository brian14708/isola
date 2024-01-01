use routes::State;
use server::serve;

mod memory_buffer;
mod resource;
mod routes;
mod server;
mod vm;
mod vm_cache;
mod vm_manager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let state = State::new("wasm/target/promptkit_python.wasm")?;
    let app = routes::router(state);
    serve(app, 3000).await
}
