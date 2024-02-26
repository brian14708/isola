#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use proto::script::script_service_server::ScriptServiceServer;

mod hybrid;
mod proto;
mod routes;
mod server;
mod service;
mod utils;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let state = routes::AppState::new("wasm/target/promptkit_python.wasm")?;
    let app = routes::router(&state);

    let service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::script::FILE_DESCRIPTOR_SET)
        .build()
        .unwrap();

    let grpc = tonic::transport::Server::builder()
        .add_service(service)
        .add_service(ScriptServiceServer::new(service::ScriptServer::new(state)))
        .into_service();

    server::serve(app, grpc, 3000).await
}
