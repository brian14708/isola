use std::sync::Arc;

use anyhow::anyhow;
use bytes::Bytes;
use parking_lot::Mutex;
use wasmtime_wasi::preview2::{
    HostOutputStream, StdoutStream, StreamError, StreamResult, Subscribe,
};

#[derive(Clone)]
pub struct MemoryOutput {
    buffer: MemoryOutputBuffer,
}

#[allow(dead_code)]
impl MemoryOutput {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: MemoryOutputBuffer {
                capacity,
                buffer: Arc::new(Mutex::new(Vec::new())),
            },
        }
    }

    pub fn pop(&self) -> anyhow::Result<String> {
        let mut buf = self.buffer.buffer.lock();
        Ok(String::from_utf8(std::mem::take(&mut *buf))?)
    }
}

#[derive(Clone)]
struct MemoryOutputBuffer {
    capacity: usize,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl StdoutStream for MemoryOutput {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.buffer.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

impl HostOutputStream for MemoryOutputBuffer {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let mut buf = self.buffer.lock();
        if bytes.len() > self.capacity - buf.len() {
            return Err(StreamError::Trap(anyhow!(
                "write beyond capacity of MemoryOutputPipe"
            )));
        }
        buf.extend_from_slice(bytes.as_ref());
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        let consumed = self.buffer.lock().len();
        if consumed < self.capacity {
            Ok(self.capacity - consumed)
        } else {
            Err(StreamError::Closed)
        }
    }
}

#[async_trait::async_trait]
impl Subscribe for MemoryOutputBuffer {
    async fn ready(&mut self) {}
}
