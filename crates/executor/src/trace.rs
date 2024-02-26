use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicI16, Ordering},
        Arc,
    },
};

use coarsetime::{Duration, Instant};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use crate::atomic_cell::AtomicCell;

pub enum TraceEventKind {
    Log {
        content: String,
    },
    Event {
        parent_id: Option<i16>,
        kind: Cow<'static, str>,
        data: Option<Box<RawValue>>,
    },
    SpanBegin {
        parent_id: Option<i16>,
        kind: Cow<'static, str>,
        data: Option<Box<RawValue>>,
    },
    SpanEnd {
        parent_id: i16,
        data: Option<Box<RawValue>>,
    },
}

pub struct TraceEvent {
    pub id: i16,
    pub group: &'static str,
    pub timestamp: Duration,
    pub kind: TraceEventKind,
}

pub type BoxedTracer = Box<dyn Tracer + Send + Sync>;
pub type TracerContext = AtomicCell<BoxedTracer>;

#[async_trait::async_trait]
pub trait Tracer {
    async fn log(&self, group: &'static str, s: Cow<'_, str>);

    async fn span_begin(
        &self,
        group: &'static str,
        parent: Option<i16>,
        kind: Cow<'static, str>,
        data: Option<Box<RawValue>>,
    ) -> i16;

    async fn event(
        &self,
        group: &'static str,
        parent: Option<i16>,
        kind: Cow<'static, str>,
        data: Option<Box<RawValue>>,
    );

    async fn span_end(&self, group: &'static str, id: i16, data: Option<Box<RawValue>>);
}

#[derive(Clone)]
pub struct MemoryTracer {
    id: Arc<AtomicI16>,
    events: mpsc::Sender<TraceEvent>,
    epoch: Instant,
}

impl MemoryTracer {
    fn next_id(&self) -> i16 {
        self.id.fetch_add(1, Ordering::Relaxed)
    }

    async fn record(&self, event: TraceEvent) {
        let _ = self.events.send(event).await;
    }
}

#[async_trait::async_trait]
impl Tracer for MemoryTracer {
    async fn log(&self, group: &'static str, s: Cow<'_, str>) {
        self.record(TraceEvent {
            id: self.next_id(),
            group,
            timestamp: self.epoch.elapsed(),
            kind: TraceEventKind::Log {
                content: s.into_owned(),
            },
        })
        .await;
    }

    async fn event(
        &self,
        group: &'static str,
        parent_id: Option<i16>,
        kind: Cow<'static, str>,
        data: Option<Box<RawValue>>,
    ) {
        self.record(TraceEvent {
            id: self.next_id(),
            group,
            timestamp: self.epoch.elapsed(),
            kind: TraceEventKind::Event {
                kind,
                parent_id,
                data,
            },
        })
        .await;
    }

    async fn span_begin(
        &self,
        group: &'static str,
        parent_id: Option<i16>,
        kind: Cow<'static, str>,
        data: Option<Box<RawValue>>,
    ) -> i16 {
        let id = self.next_id();
        self.record(TraceEvent {
            id,
            group,
            timestamp: self.epoch.elapsed(),
            kind: TraceEventKind::SpanBegin {
                kind,
                parent_id,
                data,
            },
        })
        .await;
        id
    }

    async fn span_end(&self, group: &'static str, id: i16, data: Option<Box<RawValue>>) {
        self.record(TraceEvent {
            id: self.next_id(),
            group,
            timestamp: self.epoch.elapsed(),
            kind: TraceEventKind::SpanEnd {
                parent_id: id,
                data,
            },
        })
        .await;
    }
}

impl MemoryTracer {
    #[must_use]
    pub fn new() -> (Box<Self>, mpsc::Receiver<TraceEvent>) {
        let (tx, rx) = mpsc::channel(1);
        (
            Box::new(Self {
                id: Arc::new(AtomicI16::new(1)),
                events: tx,
                epoch: Instant::now(),
            }),
            rx,
        )
    }
}
