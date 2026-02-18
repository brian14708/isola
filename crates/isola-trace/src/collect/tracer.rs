use std::sync::Arc;

use fastant::{Anchor, Instant};
use tracing::{Metadata, field::Visit, level_filters::LevelFilter, span::Attributes};

use super::{
    collector::{Collector, EventRecord, FieldFilter, SpanRecord},
    visit::FieldVisitor,
};

pub struct Tracer {
    inner: Arc<TracerInner>,
    record: SpanRecord,
    instant: Instant,
}

struct TracerInner {
    collector: Box<dyn Collector>,
    target: &'static str,
    level: LevelFilter,
    field_filter: Option<FieldFilter>,
    anchor: Anchor,
}

impl Tracer {
    pub fn new<C>(collector: C, target: &'static str, level: LevelFilter) -> Self
    where
        C: Collector,
    {
        let anchor = Anchor::new();
        let instant = Instant::now();
        Self {
            inner: Arc::new(TracerInner {
                collector: Box::new(collector),
                target,
                level,
                field_filter: C::field_filter(),
                anchor,
            }),
            record: SpanRecord {
                span_id: 0,
                parent_id: 0,
                begin_time_unix_ns: instant.as_unix_nanos(&anchor),
                duration_ns: 0,
                name: "",
                properties: Vec::new(),
            },
            instant,
        }
    }

    pub fn new_child(&self, attrs: &Attributes<'_>) -> Option<Self> {
        if !self.enabled(attrs.metadata()) {
            return None;
        }
        let span_id = self.inner.collector.next_id();
        let instant = Instant::now();
        let record = SpanRecord {
            parent_id: self.record.span_id,
            span_id,
            begin_time_unix_ns: instant.as_unix_nanos(&self.inner.anchor),
            duration_ns: 0,
            name: attrs.metadata().name(),
            properties: Vec::new(),
        };

        let mut start = record.clone();
        if let Some(filter) = &self.inner.field_filter {
            attrs.values().record(&mut FieldVisitor::new(
                |name| filter.enabled(name),
                |name, value| {
                    start.properties.push((name, value));
                },
            ));
        } else {
            attrs.values().record(&mut FieldVisitor::new(
                |_| true,
                |name, value| {
                    start.properties.push((name, value));
                },
            ));
        }
        self.inner.collector.on_span_start(start);

        Some(Self {
            inner: self.inner.clone(),
            record,
            instant,
        })
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= self.inner.level && metadata.target() == self.inner.target
    }

    pub fn finalize(mut self) {
        if self.record.span_id == 0 {
            return;
        }
        self.record.duration_ns = self
            .instant
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        self.inner.collector.on_span_end(self.record);
    }

    pub fn record_fields(&mut self, f: impl FnOnce(&mut dyn Visit)) {
        if let Some(filter) = &self.inner.field_filter {
            f(&mut FieldVisitor::new(
                |name| filter.enabled(name),
                |name, value| {
                    self.record.properties.push((name, value));
                },
            ));
        } else {
            f(&mut FieldVisitor::new(
                |_| true,
                |name, value| {
                    self.record.properties.push((name, value));
                },
            ));
        }
    }

    pub fn record_event(&self, metadata: &Metadata<'_>, f: impl FnOnce(&mut dyn Visit)) {
        if !self.enabled(metadata) {
            return;
        }
        let mut e = EventRecord {
            parent_span_id: self.record.span_id,
            name: metadata.name(),
            timestamp_unix_ns: Instant::now().as_unix_nanos(&self.inner.anchor),
            properties: vec![],
        };
        if let Some(filter) = &self.inner.field_filter {
            f(&mut FieldVisitor::new(
                |name| filter.enabled(name),
                |name, value| {
                    e.properties.push((name, value));
                },
            ));
        } else {
            f(&mut FieldVisitor::new(
                |_| true,
                |name, value| {
                    e.properties.push((name, value));
                },
            ));
        }
        self.inner.collector.on_event(e);
    }
}
