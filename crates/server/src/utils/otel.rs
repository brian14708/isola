use isola::{
    TRACE_TARGET_SCRIPT,
    trace::{collect::CollectLayer, consts::TRACE_TARGET_OTEL},
};
use opentelemetry::{
    KeyValue, global,
    propagation::TextMapCompositePropagator,
    trace::{Tracer, TracerProvider, noop::NoopTracer},
};
use opentelemetry_otlp::{
    OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_PROTOCOL, OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
    WithExportConfig,
};
use opentelemetry_sdk::{
    Resource,
    propagation::{BaggagePropagator, TraceContextPropagator},
    runtime,
    trace::{RandomIdGenerator, Sampler, span_processor_with_async_runtime::BatchSpanProcessor},
};
use opentelemetry_semantic_conventions::resource;
use tracing::{Level, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::{
    Layer, filter::FilterFn, layer::SubscriberExt, registry::LookupSpan, util::SubscriberInitExt,
};

fn get_env_var(names: &[&'static str]) -> Option<String> {
    for name in names {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

pub fn init_tracing() -> anyhow::Result<ProviderGuard> {
    let provider = if let Some(_endpoint) = get_env_var(&[
        OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
        OTEL_EXPORTER_OTLP_ENDPOINT,
    ]) {
        global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
            Box::new(TraceContextPropagator::new()),
            Box::new(BaggagePropagator::new()),
        ]));

        let protocol = get_env_var(&[
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
            OTEL_EXPORTER_OTLP_PROTOCOL,
        ])
        .unwrap_or_default();

        let exporter = match protocol.as_str() {
            "grpc" => opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_protocol(opentelemetry_otlp::Protocol::Grpc)
                .build(),
            "http/json" => opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_protocol(opentelemetry_otlp::Protocol::HttpJson)
                .build(),
            _ => opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
                .build(),
        }?;

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_span_processor(BatchSpanProcessor::builder(exporter, runtime::Tokio).build())
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOff)))
            .with_id_generator(RandomIdGenerator::default())
            .with_resource(
                Resource::builder()
                    .with_attribute(KeyValue::new(resource::SERVICE_NAME, "isola-server"))
                    .build(),
            )
            .build();

        global::set_tracer_provider(provider.clone());
        Some(provider)
    } else {
        None
    };

    let envfilter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env()
        .expect("failed to read env filter")
        .add_directive(format!("{TRACE_TARGET_SCRIPT}=off").parse().unwrap())
        .add_directive("rmcp=warn".parse().unwrap());

    let registry = tracing_subscriber::Registry::default()
        .with(
            CollectLayer::default().with_filter(FilterFn::new(|metadata| {
                metadata.target() == TRACE_TARGET_SCRIPT
            })),
        )
        .with(tracing_subscriber::fmt::Layer::default().with_filter(envfilter));

    match &provider {
        Some(provider) => registry
            .with(otel_layer(provider.tracer("isola-server")))
            .init(),
        None => registry.with(otel_layer(NoopTracer::new())).init(),
    }

    Ok(ProviderGuard(provider))
}

fn otel_layer<S, T>(tracer: T) -> impl Layer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    T: Tracer + 'static,
    T::Span: Send + Sync,
{
    tracing_opentelemetry::OpenTelemetryLayer::new(tracer)
        .with_location(false)
        .with_tracked_inactivity(false)
        .with_threads(false)
        .with_filter(FilterFn::new(|metadata| {
            *metadata.level() <= LevelFilter::INFO
                && (metadata.target() == TRACE_TARGET_SCRIPT
                    || metadata.target() == TRACE_TARGET_OTEL)
        }))
}

pub struct ProviderGuard(Option<opentelemetry_sdk::trace::SdkTracerProvider>);

impl Drop for ProviderGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.0.take() {
            let _ = provider.shutdown();
        }
    }
}
