use opentelemetry::{
    KeyValue, global,
    propagation::TextMapCompositePropagator,
    trace::{Tracer, TracerProvider, noop::NoopTracer},
};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    propagation::{BaggagePropagator, TraceContextPropagator},
    runtime,
    trace::{RandomIdGenerator, Sampler, span_processor_with_async_runtime::BatchSpanProcessor},
};
use opentelemetry_semantic_conventions::resource;
use promptkit_trace::{
    collect::CollectorLayer,
    consts::{TRACE_TARGET_OTEL, TRACE_TARGET_SCRIPT},
};
use tracing::{Level, Subscriber, level_filters::LevelFilter};
use tracing_opentelemetry::PreSampledTracer;
use tracing_subscriber::{
    Layer, filter::FilterFn, layer::SubscriberExt, registry::LookupSpan, util::SubscriberInitExt,
};

pub fn init_tracing() -> anyhow::Result<ProviderGuard> {
    global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
        Box::new(TraceContextPropagator::new()),
        Box::new(BaggagePropagator::new()),
    ]));

    let provider = if let Ok(e) = std::env::var("OTEL_COLLECTOR_URL") {
        let e = {
            // compatibility with old env var
            let mut u = url::Url::parse(&e).expect("OTEL_COLLECTOR_URL is not a valid URL");
            if u.path() == "/" {
                u = u.join("/v1/traces").expect("failed to append /v1/traces");
            }
            u.to_string()
        };

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_span_processor(
                BatchSpanProcessor::builder(
                    opentelemetry_otlp::SpanExporter::builder()
                        .with_http()
                        .with_endpoint(e)
                        .build()?,
                    runtime::Tokio,
                )
                .build(),
            )
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOff)))
            .with_id_generator(RandomIdGenerator::default())
            .with_resource(
                Resource::builder()
                    .with_attribute(KeyValue::new(resource::SERVICE_NAME, "promptkit"))
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
        .add_directive(format!("{TRACE_TARGET_SCRIPT}=off").parse().unwrap());
    let registry = tracing_subscriber::Registry::default()
        .with(
            CollectorLayer::default().with_filter(FilterFn::new(|metadata| {
                metadata.target() == TRACE_TARGET_SCRIPT
            })),
        )
        .with(tracing_subscriber::fmt::Layer::default().with_filter(envfilter));
    match &provider {
        Some(provider) => registry
            .with(otel_layer(provider.tracer("promptkit")))
            .init(),
        None => registry.with(otel_layer(NoopTracer::new())).init(),
    }

    Ok(ProviderGuard(provider))
}

fn otel_layer<S, T>(tracer: T) -> impl Layer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    T: Tracer + PreSampledTracer + 'static,
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
