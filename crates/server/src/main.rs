#![warn(clippy::pedantic)]
#![forbid(unsafe_code)]

use std::{env::args, path::PathBuf};

use anyhow::anyhow;
use promptkit_executor::VmManager;
use proto::script::v1::script_service_server::ScriptServiceServer;
use tonic::{codec::CompressionEncoding, service::LayerExt};
use utils::{grpc_trace::grpc_server_tracing_layer, otel::init_tracing};

use crate::routes::VmEnv;

mod proto;
mod routes;
mod server;
mod service;
mod utils;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> anyhow::Result<()> {
    if let Err(e) = rlimit::increase_nofile_limit(u64::MAX) {
        tracing::warn!("Failed to raise ulimit: {}", e);
    }

    let rt = match std::env::var("TOKIO_NUM_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(0) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,

        Some(n) if n > 0 => tokio::runtime::Builder::new_multi_thread()
            .worker_threads(n)
            .enable_all()
            .build()?,

        _ => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    };

    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let task = args().nth(1);
    match task.as_deref() {
        Some("build") => {
            VmManager::<VmEnv>::compile(&PathBuf::from("wasm/target/promptkit_python.wasm"))
                .await?;
            Ok(())
        }
        None | Some("serve") => {
            let _provider = init_tracing()?;
            let state = routes::AppState::new("wasm/target/promptkit_python.wasm").await?;
            let app = routes::router(&state);

            let grpc = tonic::service::Routes::default()
                .add_service(
                    tonic_web::GrpcWebLayer::new().named_layer(
                        ScriptServiceServer::new(service::ScriptServer::new(state))
                            .send_compressed(CompressionEncoding::Gzip)
                            .accept_compressed(CompressionEncoding::Gzip)
                            .max_decoding_message_size(usize::MAX),
                    ),
                )
                .add_service(
                    tonic_reflection::server::Builder::configure()
                        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
                        .build_v1()
                        .unwrap(),
                )
                .prepare()
                .into_axum_router()
                .layer(grpc_server_tracing_layer());

            server::serve(
                app.merge(grpc),
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
