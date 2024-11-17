#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::{env::args, path::PathBuf};

use anyhow::anyhow;
use opentelemetry::{
    global, propagation::TextMapCompositePropagator, trace::TracerProvider, KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::{BaggagePropagator, TraceContextPropagator},
    trace::{self, RandomIdGenerator, Sampler},
    Resource,
};
use opentelemetry_semantic_conventions::resource;
use otel::{grpc_server_tracing_layer, request_tracing_layer};
use promptkit_executor::VmManager;
use proto::script::v1::script_service_server::ScriptServiceServer;
use tonic::codec::CompressionEncoding;
use tracing::{level_filters::LevelFilter, Level};
use tracing_subscriber::{filter::FilterFn, layer::SubscriberExt, util::SubscriberInitExt, Layer};

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
        let e = {
            // compatibility with old env var
            let mut u = url::Url::parse(&e).expect("OTEL_COLLECTOR_URL is not a valid URL");
            if u.path() == "/" {
                u = u.join("/v1/traces").expect("failed to append /v1/traces");
            }
            u.to_string()
        };
        let provider = opentelemetry_sdk::trace::TracerProvider::builder()
            .with_batch_exporter(
                opentelemetry_otlp::SpanExporter::builder()
                    .with_http()
                    .with_endpoint(e)
                    .build()?,
                opentelemetry_sdk::runtime::Tokio,
            )
            .with_config(
                trace::Config::default()
                    .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOff)))
                    .with_id_generator(RandomIdGenerator::default())
                    .with_resource(Resource::new(vec![KeyValue::new(
                        resource::SERVICE_NAME,
                        "promptkit",
                    )])),
            )
            .build();
        global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
            Box::new(TraceContextPropagator::new()),
            Box::new(BaggagePropagator::new()),
        ]));
        let opentelemetry = tracing_opentelemetry::layer()
            .with_location(false)
            .with_tracked_inactivity(false)
            .with_threads(false)
            .with_tracer(provider.tracer("promptkit"))
            .with_filter(FilterFn::new(|metadata| {
                *metadata.level() <= LevelFilter::INFO
                    && metadata
                        .fields()
                        .iter()
                        .any(|field| field.name() == "promptkit.user")
            }));

        tracing_subscriber::registry()
            .with(opentelemetry)
            .with(request_tracing_layer())
            .with(tracing_subscriber::fmt::Layer::default().with_filter(envfilter))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(request_tracing_layer())
            .with(tracing_subscriber::fmt::Layer::default().with_filter(envfilter))
            .init();
    }

    let task = args().nth(1);
    match task.as_deref() {
        Some("build") => {
            VmManager::<()>::compile(&PathBuf::from("wasm/target/promptkit_python.wasm")).await?;
            Ok(())
        }
        None | Some("serve") => {
            let state = routes::AppState::new("wasm/target/promptkit_python.wasm")?;
            let app = routes::router(&state);

            let grpc = tonic::service::Routes::new(tonic_web::enable(
                ScriptServiceServer::new(service::ScriptServer::new(state))
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip),
            ))
            .add_service(
                tonic_reflection::server::Builder::configure()
                    .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
                    .build_v1()
                    .unwrap(),
            )
            .add_service(
                tonic_reflection::server::Builder::configure()
                    .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
                    .build_v1alpha()
                    .unwrap(),
            )
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
