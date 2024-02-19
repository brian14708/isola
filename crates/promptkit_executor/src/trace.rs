use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicI16, Ordering},
        Arc,
    },
};

use coarsetime::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Clone, Copy)]
pub enum TraceLogLevel {
    Stdout,
    Stderr,
}

pub enum TraceEventKind {
    Log {
        level: TraceLogLevel,
        content: String,
        timestamp: Duration,
    },
}

pub struct TraceEvent {
    pub id: i16,
    pub kind: TraceEventKind,
}

#[async_trait::async_trait]
pub trait Tracer {
    async fn log(&self, lvl: TraceLogLevel, s: Cow<'_, str>);

    fn boxed(self) -> Box<dyn Tracer + Send + Sync>;
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
}

#[async_trait::async_trait]
impl Tracer for MemoryTracer {
    fn boxed(self) -> Box<dyn Tracer + Send + Sync> {
        Box::new(self)
    }

    async fn log(&self, level: TraceLogLevel, s: Cow<'_, str>) {
        let _ = self
            .events
            .send(TraceEvent {
                id: self.next_id(),
                kind: TraceEventKind::Log {
                    level,
                    content: s.into_owned(),
                    timestamp: self.epoch.elapsed(),
                },
            })
            .await;
    }
}

impl MemoryTracer {
    pub fn new() -> (Self, mpsc::Receiver<TraceEvent>) {
        let (tx, rx) = mpsc::channel(1);
        (
            Self {
                id: Arc::new(AtomicI16::new(0)),
                events: tx,
                epoch: Instant::now(),
            },
            rx,
        )
    }
}
