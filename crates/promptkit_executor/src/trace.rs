use std::{
    borrow::Cow,
    ops::DerefMut,
    sync::{
        atomic::{AtomicI16, Ordering},
        Arc,
    },
};

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
    },
}

pub trait Tracer: Send + Sync {
    fn next_id(&self) -> i16;

    fn box_clone(&self) -> Box<dyn Tracer>;

    fn log(&self, lvl: TraceLogLevel, s: Cow<'_, str>);
}

#[derive(Clone)]
pub struct MemoryTracer {
    inner: Arc<MemoryTracerInner>,
}

struct MemoryTracerInner {
    id: AtomicI16,
    events: RwLock<SmallVec<[TraceEvent; 8]>>,
}

impl Tracer for MemoryTracer {
    fn next_id(&self) -> i16 {
        self.inner.id.fetch_add(1, Ordering::Relaxed)
    }

    fn log(&self, level: TraceLogLevel, s: Cow<'_, str>) {
        self.inner.events.write().push(TraceEvent::Log {
            level,
            content: s.to_string(),
        });
    }

    fn box_clone(&self) -> Box<dyn Tracer> {
        Box::new(self.clone())
    }
}

impl MemoryTracer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoryTracerInner {
                id: AtomicI16::new(0),
                events: RwLock::new(SmallVec::new()),
            }),
        }
    }

    pub fn events(self) -> impl IntoIterator<Item = TraceEvent> {
        std::mem::take(self.inner.events.write().deref_mut()).into_iter()
    }
}

impl Default for MemoryTracer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default, Clone)]
pub struct NoopTracer();

impl Tracer for NoopTracer {
    fn next_id(&self) -> i16 {
        0
    }

    fn log(&self, _: TraceLogLevel, _: Cow<'_, str>) {}

    fn box_clone(&self) -> Box<dyn Tracer> {
        Box::new(self.clone())
    }
}
