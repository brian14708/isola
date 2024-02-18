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

pub enum TraceEvent {
    Log {
        level: TraceLogLevel,
        content: String,
        timestamp: Duration,
    },
}

#[async_trait::async_trait]
pub trait Logger {
    async fn log(&self, lvl: TraceLogLevel, s: Cow<'_, str>);
}

pub trait Tracer: Logger + Send + Sync {
    fn next_id(&self) -> i16;

    fn boxed_logger(&self) -> Box<dyn Logger>;
}

#[derive(Clone)]
pub struct MemoryTracer {
    inner: Arc<MemoryTracerInner>,
}

struct MemoryTracerInner {
    id: AtomicI16,
    events: mpsc::Sender<TraceEvent>,
    epoch: Instant,
}

impl Tracer for MemoryTracer {
    fn next_id(&self) -> i16 {
        self.inner.id.fetch_add(1, Ordering::Relaxed)
    }

    fn boxed_logger(&self) -> Box<dyn Logger> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Logger for MemoryTracer {
    async fn log(&self, level: TraceLogLevel, s: Cow<'_, str>) {
        let _ = self
            .inner
            .events
            .send(TraceEvent::Log {
                level,
                content: s.to_string(),
                timestamp: self.inner.epoch.elapsed(),
            })
            .await;
    }
}

impl MemoryTracer {
    pub fn new() -> (Self, mpsc::Receiver<TraceEvent>) {
        let (tx, rx) = mpsc::channel(1);
        (
            Self {
                inner: Arc::new(MemoryTracerInner {
                    id: AtomicI16::new(0),
                    events: tx,
                    epoch: Instant::now(),
                }),
            },
            rx,
        )
    }
}
