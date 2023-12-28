use server::serve;

mod error;
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

    let app = routes::router()?;
    serve(app, 3000).await
}
