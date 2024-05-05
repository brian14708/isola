#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::{env::args, path::PathBuf};

use anyhow::anyhow;
use opentelemetry::{global, propagation::TextMapCompositePropagator, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::{BaggagePropagator, TraceContextPropagator},
    trace::{self, RandomIdGenerator, Sampler},
    Resource,
};
use opentelemetry_semantic_conventions::resource;
use otel::grpc_server_tracing_layer;
use promptkit_executor::VmManager;
use proto::script::script_service_server::ScriptServiceServer;
use tonic::codec::CompressionEncoding;
use tracing::Level;
use tracing_subscriber::{filter::FilterFn, layer::SubscriberExt, util::SubscriberInitExt, Layer};

mod hybrid;
mod otel;
mod proto;
mod routes;
mod server;
mod service;
mod utils;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let envfilter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env()
        .expect("failed to read env filter")
        .add_directive("[{promptkit.user}]=off".parse().unwrap());

    if let Ok(e) = std::env::var("OTEL_COLLECTOR_URL") {
        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(opentelemetry_otlp::new_exporter().http().with_endpoint(e))
            .with_trace_config(
                trace::config()
                    .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOff)))
                    .with_id_generator(RandomIdGenerator::default())
                    .with_resource(Resource::new(vec![KeyValue::new(
                        resource::SERVICE_NAME,
                        "promptkit",
                    )])),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio)?;
        global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
            Box::new(TraceContextPropagator::new()),
            Box::new(BaggagePropagator::new()),
        ]));
        let opentelemetry = tracing_opentelemetry::layer()
            .with_location(false)
            .with_tracked_inactivity(false)
            .with_threads(false)
            .with_tracer(tracer)
            .with_filter(FilterFn::new(|metadata| {
                metadata
                    .fields()
                    .iter()
                    .any(|field| field.name() == "promptkit.user")
            }));

        tracing_subscriber::registry()
            .with(opentelemetry)
            .with(tracing_subscriber::fmt::Layer::default().with_filter(envfilter))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::Layer::default().with_filter(envfilter))
            .init();
    }

    let task = args().nth(1);
    match task.as_deref() {
        Some("build") => {
            VmManager::<()>::compile(&PathBuf::from("wasm/target/promptkit_python.wasm"))?;
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
                .layer(grpc_server_tracing_layer())
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
