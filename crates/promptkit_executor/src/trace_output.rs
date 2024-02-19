use std::sync::Arc;

use bytes::Bytes;
use wasmtime_wasi::preview2::{HostOutputStream, StdoutStream, StreamResult, Subscribe};

use crate::{
    atomic_cell::AtomicCell,
    trace::{TraceLogLevel, Tracer},
};

pub struct TraceOutput {
    ctx: Arc<AtomicCell<Box<dyn Tracer + Send + Sync>>>,
    level: TraceLogLevel,
}

impl TraceOutput {
    pub fn new(ctx: Arc<AtomicCell<Box<dyn Tracer + Send + Sync>>>, level: TraceLogLevel) -> Self {
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
            prev_write: vec![],
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    ctx: Arc<AtomicCell<Box<dyn Tracer + Send + Sync>>>,
    level: TraceLogLevel,
    buffer: Vec<u8>,
    prev_write: Vec<u8>,
}

const MAX_BUFFER: usize = 1024;

#[async_trait::async_trait]
impl Subscribe for TraceOutputStream {
    async fn ready(&mut self) {
        if self.prev_write.is_empty() {
            return;
        }

        let s = String::from_utf8_lossy(&self.prev_write);
        self.ctx.with_async(|t| t.log(self.level, s)).await;
        self.prev_write.clear()
    }
}

impl HostOutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if !self.ctx.is_null() {
            self.buffer.extend(bytes);
            if self.buffer.len() >= MAX_BUFFER {
                self.prev_write = std::mem::take(&mut self.buffer);
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if !self.ctx.is_null() {
            self.prev_write = std::mem::take(&mut self.buffer);
        } else {
            self.buffer.clear();
        }
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if !self.prev_write.is_empty() {
            Ok(0)
        } else {
            Ok(MAX_BUFFER - self.buffer.len())
        }
    }
}
