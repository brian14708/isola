#![warn(clippy::pedantic)]

use super::collector::Collector;
use super::tracer::Tracer;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{Registry, registry::LookupSpan};

pub trait CollectorSpanExt {
    #[must_use]
    fn collect_into(
        &self,
        target: &'static str,
        level: LevelFilter,
        collector: impl Collector,
    ) -> Option<()>;
}

impl CollectorSpanExt for tracing::Span {
    fn collect_into(
        &self,
        target: &'static str,
        level: LevelFilter,
        c: impl Collector,
    ) -> Option<()> {
        self.with_subscriber(|(id, subscriber)| -> Option<()> {
            if let Some(registry) = subscriber.downcast_ref::<Registry>() {
                match registry.span(id) {
                    Some(span) => {
                        if span
                            .scope()
                            .any(|s| s.extensions().get::<Tracer>().is_some())
                        {
                            // nesting is not supported
                            None
                        } else {
                            span.extensions_mut().insert(Tracer::new(c, target, level));
                            Some(())
                        }
                    }
                    _ => None,
                }
            } else {
                None
            }
        })
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::{info, info_span};
    use tracing_subscriber::{Registry, layer::SubscriberExt};

    use super::super::{
        collector::{EventRecord, SpanRecord},
        layer::CollectorLayer,
    };
    use super::*;

    fn with_layer<T>(f: impl FnOnce() -> T) -> T {
        tracing::subscriber::with_default(Registry::default().with(CollectorLayer::default()), f)
    }

    #[derive(Clone)]
    struct VecCollector(Arc<Mutex<(Vec<SpanRecord>, Vec<EventRecord>)>>);
    impl Collector for VecCollector {
        fn collect_span(&self, v: SpanRecord) {
            self.0.lock().unwrap().0.push(v);
        }
        fn collect_event(&self, v: EventRecord) {
            self.0.lock().unwrap().1.push(v);
        }
    }

    #[test]
    fn test_span() {
        with_layer(|| {
            let s = info_span!("hello");
            let spans = VecCollector(Arc::new(Mutex::new((Vec::new(), Vec::new()))));
            s.collect_into("a", LevelFilter::INFO, spans.clone())
                .unwrap();
            {
                let _s = s.enter();

                let xx = info_span!(target: "a", "xx", a = 12);
                let _xx = xx.enter();
                let xx = info_span!(target: "b", "xx");
                let _xx = xx.enter();

                info!(target: "a", "hello");
                info!(target: "b", "hello");
            }
            let s = spans.0.lock().unwrap();
            let (spans, events) = (&s.0, &s.1);
            assert_eq!(1, spans.len());
            assert_eq!(1, events.len());
        });
    }
}
