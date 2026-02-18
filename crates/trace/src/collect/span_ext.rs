#![warn(clippy::pedantic)]

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{Registry, registry::LookupSpan};

use super::{collector::Collector, tracer::Tracer};

pub trait CollectSpanExt {
    #[must_use]
    fn collect_into(
        &self,
        target: &'static str,
        level: LevelFilter,
        collector: impl Collector,
    ) -> Option<()>;
}

impl CollectSpanExt for tracing::Span {
    fn collect_into(
        &self,
        target: &'static str,
        level: LevelFilter,
        c: impl Collector,
    ) -> Option<()> {
        self.with_subscriber(|(id, subscriber)| -> Option<()> {
            subscriber.downcast_ref::<Registry>().and_then(|registry| {
                registry.span(id).and_then(|span| {
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
                })
            })
        })
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::{info, info_span};
    use tracing_subscriber::{Registry, layer::SubscriberExt};

    use super::{
        super::{
            collector::{EventRecord, SpanRecord},
            layer::CollectLayer,
        },
        *,
    };

    fn with_layer<T>(f: impl FnOnce() -> T) -> T {
        tracing::subscriber::with_default(Registry::default().with(CollectLayer::default()), f)
    }

    #[derive(Clone)]
    struct VecCollector(Arc<Mutex<(Vec<SpanRecord>, Vec<EventRecord>)>>);
    impl Collector for VecCollector {
        fn on_span_start(&self, v: SpanRecord) {
            self.0.lock().unwrap().0.push(v);
        }
        fn on_span_end(&self, _v: SpanRecord) {}
        fn on_event(&self, v: EventRecord) {
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
            let (span_len, event_len) = (s.0.len(), s.1.len());
            drop(s);
            assert_eq!(1, span_len);
            assert_eq!(1, event_len);
        });
    }
}
