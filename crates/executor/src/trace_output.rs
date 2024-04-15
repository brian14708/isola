use std::sync::Arc;

use bytes::Bytes;
use wasmtime_wasi::{HostOutputStream, StdoutStream, StreamResult, Subscribe};

use crate::trace::TracerContext;

pub struct TraceOutput {
    ctx: Arc<TracerContext>,
    group: &'static str,
}

impl TraceOutput {
    pub fn new(ctx: Arc<TracerContext>, group: &'static str) -> Self {
        Self { ctx, group }
    }
}

impl StdoutStream for TraceOutput {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(TraceOutputStream {
            ctx: self.ctx.clone(),
            group: self.group,
            buffer: vec![],
            prev_write: vec![],
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    ctx: Arc<TracerContext>,
    group: &'static str,
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

        let s = String::from_utf8(std::mem::take(&mut self.prev_write));
        match s {
            Ok(s) => {
                self.ctx.with_async(|t| t.log(self.group, s.into())).await;
            }
            Err(e) => {
                let error = e.utf8_error();
                let mut input = e.into_bytes();
                if input.len() - error.valid_up_to() > 4 {
                    // lossy
                    let s = String::from_utf8_lossy(&input);
                    self.ctx.with_async(|t| t.log(self.group, s.into())).await;
                } else {
                    self.prev_write.extend(input.drain(error.valid_up_to()..));
                    if input.is_empty() {
                        return;
                    }

                    let s = unsafe { String::from_utf8_unchecked(input) };
                    self.ctx.with_async(|t| t.log(self.group, s.into())).await;
                }
            }
        }
    }
}

impl HostOutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if !self.ctx.is_null() {
            self.buffer.extend(bytes);
            if self.buffer.len() >= MAX_BUFFER {
                self.prev_write.extend(std::mem::take(&mut self.buffer));
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if self.ctx.is_null() {
            self.buffer.clear();
        } else {
            self.prev_write.extend(std::mem::take(&mut self.buffer));
        }
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if MAX_BUFFER > (self.buffer.len() + self.prev_write.len()) {
            Ok(MAX_BUFFER - self.buffer.len() + self.prev_write.len())
        } else {
            Ok(0)
        }
    }
}
