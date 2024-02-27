#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::{env::args, path::PathBuf};

use anyhow::anyhow;
use promptkit_executor::VmManager;
use proto::script::script_service_server::ScriptServiceServer;
use tonic::codec::CompressionEncoding;

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

    let task = args().nth(1);
    match task.as_deref() {
        Some("build") => {
            VmManager::compile(&PathBuf::from("wasm/target/promptkit_python.wasm"))?;
            Ok(())
        }
        None | Some("serve") => {
            let state = routes::AppState::new("wasm/target/promptkit_python.wasm")?;
            let app = routes::router(&state);

            let service = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(proto::script::FILE_DESCRIPTOR_SET)
                .build()
                .unwrap();

            let grpc = tonic::transport::Server::builder()
                .accept_http1(true)
                .add_service(service)
                .add_service(tonic_web::enable(
                    ScriptServiceServer::new(service::ScriptServer::new(state))
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip),
                ))
                .into_service();

            server::serve(app, grpc, 3000).await
        }
        _ => Err(anyhow!("unknown task")),
    }
}
