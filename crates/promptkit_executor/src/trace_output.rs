use std::{
    ptr::null_mut,
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc,
    },
};

use bytes::Bytes;
use wasmtime_wasi::preview2::{HostOutputStream, StdoutStream, StreamResult, Subscribe};

use crate::trace::{Logger, TraceLogLevel};

#[derive(Clone)]
pub struct TraceContext {
    inner: Arc<TraceContextInner>,
}

struct TraceContextInner {
    ptr: AtomicPtr<Box<dyn Logger>>,
}

impl TraceContext {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(TraceContextInner {
                ptr: AtomicPtr::new(null_mut()),
            }),
        }
    }

    pub fn set(&mut self, b: Option<Box<dyn Logger>>) {
        self.inner.set(b)
    }
}

impl TraceContextInner {
    fn set(&self, b: Option<Box<dyn Logger>>) {
        let old = self.ptr.swap(
            b.map(|b| Box::into_raw(Box::new(b)))
                .unwrap_or_else(null_mut),
            Ordering::Release,
        );
        if !old.is_null() {
            drop(unsafe { Box::from_raw(old) });
        }
    }

    fn is_null(&self) -> bool {
        self.ptr.load(Ordering::Relaxed).is_null()
    }

    fn get(&self) -> Option<&dyn Logger> {
        let m = self.ptr.load(Ordering::Acquire);
        (!m.is_null()).then(|| unsafe { (*m).as_ref() })
    }
}

impl Drop for TraceContextInner {
    fn drop(&mut self) {
        self.set(None)
    }
}

pub struct TraceOutput {
    ctx: TraceContext,
    level: TraceLogLevel,
}

impl TraceOutput {
    pub fn new(ctx: &TraceContext, level: TraceLogLevel) -> Self {
        Self {
            ctx: ctx.clone(),
            level,
        }
    }
}

impl StdoutStream for TraceOutput {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(TraceOutputStream {
            ctx: self.ctx.clone(),
            level: self.level,
            buffer: vec![],
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    ctx: TraceContext,
    level: TraceLogLevel,
    buffer: Vec<u8>,
}

#[async_trait::async_trait]
impl Subscribe for TraceOutputStream {
    async fn ready(&mut self) {}
}

impl HostOutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if !self.ctx.inner.is_null() {
            self.buffer.extend(bytes);
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if let Some(t) = self.ctx.inner.get() {
            t.log(self.level, String::from_utf8_lossy(&self.buffer));
        }
        self.buffer.clear();
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(1024)
    }
}
