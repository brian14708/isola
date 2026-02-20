use std::marker::PhantomData;

use tracing::{Subscriber, span};
use tracing_subscriber::{Layer, registry::LookupSpan};

use super::tracer::Tracer;

pub struct CollectLayer<S> {
    _inner: PhantomData<S>,
}

impl<S> Default for CollectLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn default() -> Self {
        Self {
            _inner: PhantomData,
        }
    }
}

impl<S> Layer<S> for CollectLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        for parent in span.scope().skip(1) {
            if let Some(tracer) = parent.extensions().get::<Tracer>() {
                if let Some(tracer) = tracer.new_child(attrs) {
                    span.extensions_mut().insert(tracer);
                }
                return;
            }
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = event.parent().and_then(|id| ctx.span(id)).or_else(|| {
            event
                .is_contextual()
                .then(|| ctx.lookup_current())
                .flatten()
        }) else {
            return;
        };

        for parent in span.scope() {
            if let Some(tracer) = parent.extensions_mut().get_mut::<Tracer>() {
                tracer.record_event(event.metadata(), |visit| event.record(visit));
                return;
            }
        }
    }

    fn on_record(
        &self,
        id: &span::Id,
        values: &span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if let Some(tracer) = span.extensions_mut().get_mut::<Tracer>() {
            tracer.record_fields(|visit| values.record(visit));
        }
    }

    fn on_close(&self, id: span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let mut ext = span.extensions_mut();
        if let Some(tracer) = ext.remove::<Tracer>() {
            tracer.finalize();
        }
    }
}
