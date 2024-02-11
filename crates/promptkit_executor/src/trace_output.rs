use std::sync::Arc;

use bytes::Bytes;
use parking_lot::RwLock;
use wasmtime_wasi::preview2::{HostOutputStream, StdoutStream, StreamResult, Subscribe};

use crate::{trace::TraceLogLevel, trace::Tracer};

pub struct TraceContext {
    current: Arc<RwLock<Option<Box<dyn Tracer>>>>,
}

impl TraceContext {
    pub fn new() -> Self {
        Self {
            current: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set(&self, b: Box<dyn Tracer>) {
        *self.current.write() = Some(b);
    }

    pub fn unset(&self) {
        *self.current.write() = None;
    }
}

pub struct TraceOutput {
    current: Arc<RwLock<Option<Box<dyn Tracer>>>>,
    level: TraceLogLevel,
}

impl TraceOutput {
    pub fn new(ctx: &TraceContext, level: TraceLogLevel) -> Self {
        Self {
            current: ctx.current.clone(),
            level,
        }
    }
}

impl StdoutStream for TraceOutput {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(TraceOutputStream {
            current: self.current.clone(),
            level: self.level,
            buffer: vec![],
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    current: Arc<RwLock<Option<Box<dyn Tracer>>>>,
    level: TraceLogLevel,
    buffer: Vec<u8>,
}

#[async_trait::async_trait]
impl Subscribe for TraceOutputStream {
    async fn ready(&mut self) {}
}

impl HostOutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.buffer.extend(bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        let t = self.current.read();
        if let Some(t) = t.as_ref() {
            t.log(self.level, String::from_utf8_lossy(&self.buffer));
            self.buffer.clear();
        }
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(1024)
    }
}
