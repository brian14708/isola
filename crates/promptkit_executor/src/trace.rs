use std::{
    borrow::Cow,
    ops::DerefMut,
    sync::{
        atomic::{AtomicI16, Ordering},
        Arc,
    },
};

use coarsetime::{Duration, Instant};
use parking_lot::RwLock;
use smallvec::SmallVec;

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

pub trait Logger {
    fn log(&self, lvl: TraceLogLevel, s: Cow<'_, str>);
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
    events: RwLock<SmallVec<[TraceEvent; 8]>>,
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

impl Logger for MemoryTracer {
    fn log(&self, level: TraceLogLevel, s: Cow<'_, str>) {
        self.inner.events.write().push(TraceEvent::Log {
            level,
            content: s.to_string(),
            timestamp: self.inner.epoch.elapsed(),
        });
    }
}

impl MemoryTracer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoryTracerInner {
                id: AtomicI16::new(0),
                events: RwLock::new(SmallVec::new()),
                epoch: Instant::now(),
            }),
        }
    }

    pub fn events(self) -> impl Iterator<Item = TraceEvent> {
        std::mem::take(self.inner.events.write().deref_mut()).into_iter()
    }
}

impl Default for MemoryTracer {
    fn default() -> Self {
        Self::new()
    }
}
