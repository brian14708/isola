use std::{
    collections::HashMap,
    marker::PhantomData,
    ops::Add,
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use coarsetime::Instant;
use tokio::sync::mpsc;
use tracing::{level_filters::LevelFilter, span, Metadata, Subscriber};
use tracing_subscriber::{filter::FilterFn, registry::LookupSpan, Layer, Registry};

use crate::proto::script::{trace, Trace};

use super::visit::{FieldVisitor, StringVisitor, VisitExt};

struct RequestTracingLayer<S> {
    _inner: PhantomData<S>,
}

pub fn request_tracing_layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    RequestTracingLayer {
        _inner: PhantomData,
    }
    .with_filter(FilterFn::new(|metadata| {
        metadata
            .fields()
            .iter()
            .any(|field| field.name() == "promptkit.user")
    }))
}

impl<S> Layer<S> for RequestTracingLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        if let Some(parent) = span.scope().skip(1).find(|f| {
            f.fields()
                .iter()
                .any(|field| field.name() == "promptkit.user")
        }) {
            if let Some(tracer) = parent.extensions().get::<MemoryTracer>() {
                let mut ext = span.extensions_mut();
                if *span.metadata().level() > tracer.request.level_filter {
                    ext.insert(tracer.clone_passthrough());
                    return;
                }

                let (child, mut trace) = tracer.new_child(span.metadata());
                let mut data = trace::SpanBegin {
                    parent_id: tracer.id,
                    kind: span.name().to_string(),
                    data: HashMap::new(),
                };
                attrs.record(&mut StringVisitor::new(|name, value| {
                    data.data.insert(name.to_string(), value);
                }));
                trace.trace_type = Some(trace::TraceType::SpanBegin(data));

                tracer.send(trace);
                ext.insert(child);
            }
        }
    }

    fn on_record(
        &self,
        id: &span::Id,
        values: &span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut ext = span.extensions_mut();
        let tracer = ext.get_mut::<MemoryTracer>();
        if let Some(tracer) = tracer {
            if *span.metadata().level() > tracer.request.level_filter {
                return;
            }
            values.record(&mut StringVisitor::new(|name, value| {
                tracer.span_data.push((name, value));
            }));
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = event
            .parent()
            .and_then(|id| ctx.span(id))
            .or_else(|| {
                event
                    .is_contextual()
                    .then(|| ctx.lookup_current())
                    .flatten()
            })
            .and_then(|span| {
                span.scope().find(|f| {
                    f.fields()
                        .iter()
                        .any(|field| field.name() == "promptkit.user")
                })
            })
        {
            let ext = span.extensions();
            let tracer = ext.get::<MemoryTracer>();
            if let Some(tracer) = tracer {
                if *event.metadata().level() > tracer.request.level_filter {
                    return;
                }

                let mut trace = tracer.make_event(event.metadata());
                match event.fields().find_map(|f| match f.name() {
                    "promptkit.log.output" => Some(trace::TraceType::Log(trace::Log {
                        content: String::new(),
                    })),
                    "promptkit.event.kind" => Some(trace::TraceType::Event(trace::Event {
                        kind: String::new(),
                        parent_id: tracer.id,
                        data: HashMap::new(),
                    })),
                    _ => None,
                }) {
                    Some(trace::TraceType::Log(mut data)) => {
                        event.record(&mut FieldVisitor::new(
                            "promptkit.log.output",
                            &mut data.content,
                        ));
                        trace.trace_type = Some(trace::TraceType::Log(data));
                        tracer.send(trace);
                    }
                    Some(trace::TraceType::Event(mut data)) => {
                        event.record(
                            &mut FieldVisitor::new("promptkit.event.kind", &mut data.kind).chain(
                                StringVisitor::new(|name, value| {
                                    data.data.insert(name.to_string(), value);
                                }),
                            ),
                        );
                        trace.trace_type = Some(trace::TraceType::Event(data));
                        tracer.send(trace);
                    }
                    _ => (),
                }
            }
        }
    }

    fn on_close(&self, id: span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut ext = span.extensions_mut();
        if let Some(mut tracer) = ext.remove::<MemoryTracer>() {
            if *span.metadata().level() > tracer.request.level_filter {
                return;
            }
            let mut trace = tracer.make_event(span.metadata());
            let data = std::mem::take(&mut tracer.span_data)
                .into_iter()
                .map(|(a, b)| (a.to_string(), b))
                .collect::<_>();
            trace.trace_type = Some(trace::TraceType::SpanEnd(trace::SpanEnd {
                parent_id: tracer.id,
                data,
            }));
            tracer.send(trace);
        }
    }
}

pub trait RequestSpanExt {
    fn enable_tracing(&self, level: LevelFilter) -> Option<mpsc::UnboundedReceiver<Trace>>;
}

impl RequestSpanExt for tracing::Span {
    fn enable_tracing(&self, level: LevelFilter) -> Option<mpsc::UnboundedReceiver<Trace>> {
        self.with_subscriber(|(id, subscriber)| {
            if let Some(registry) = subscriber.downcast_ref::<Registry>() {
                if let Some(span) = registry.span(id) {
                    let (tx, rx) = mpsc::unbounded_channel();
                    span.extensions_mut().insert(MemoryTracer::new(tx, level));
                    Some(rx)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .flatten()
    }
}

struct RequestData {
    level_filter: LevelFilter,
    idgen: AtomicI32,
    start: Duration,
    epoch: Instant,
    events: mpsc::UnboundedSender<Trace>,
}

struct MemoryTracer {
    request: Arc<RequestData>,
    id: i32,
    span_data: Vec<(&'static str, String)>,
}

impl MemoryTracer {
    fn new(tx: mpsc::UnboundedSender<Trace>, level_filter: LevelFilter) -> Self {
        MemoryTracer {
            id: 0,
            request: Arc::new(RequestData {
                level_filter,
                idgen: AtomicI32::new(1),
                start: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default(),
                epoch: Instant::now(),
                events: tx,
            }),
            span_data: Vec::new(),
        }
    }

    fn new_child(&self, metadata: &Metadata<'_>) -> (Self, Trace) {
        let evt = self.make_event(metadata);
        (
            MemoryTracer {
                id: evt.id,
                request: self.request.clone(),
                span_data: Vec::new(),
            },
            evt,
        )
    }

    fn clone_passthrough(&self) -> Self {
        MemoryTracer {
            id: self.id,
            request: self.request.clone(),
            span_data: Vec::new(),
        }
    }

    fn make_event(&self, metadata: &Metadata<'_>) -> Trace {
        let ts = self.request.start.add(std::time::Duration::from_micros(
            self.request.epoch.elapsed().as_micros(),
        ));

        Trace {
            id: self.request.idgen.fetch_add(1, Ordering::Relaxed),
            group: metadata.target().to_string(),
            timestamp: Some(prost_types::Timestamp {
                #[allow(clippy::cast_possible_wrap)]
                seconds: ts.as_secs() as i64,
                #[allow(clippy::cast_possible_wrap)]
                nanos: ts.subsec_nanos() as i32,
            }),
            trace_type: None,
        }
    }

    fn send(&self, trace: Trace) {
        let _ = self.request.events.send(trace);
    }
}
