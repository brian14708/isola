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
            flush_buffer: vec![],
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
    flush_buffer: Vec<u8>,
}

const MAX_BUFFER: usize = 1024;

#[async_trait::async_trait]
impl Subscribe for TraceOutputStream {
    async fn ready(&mut self) {
        if self.flush_buffer.is_empty() {
            return;
        }

        match String::from_utf8(std::mem::take(&mut self.flush_buffer)) {
            Ok(s) => {
                self.ctx.with_async(|t| t.log(self.group, s.into())).await;
            }
            Err(e) => {
                let error = e.utf8_error();
                let input = e.into_bytes();
                let s = if input.len() - error.valid_up_to() > 4 {
                    // not a valid utf-8 sequence
                    String::from_utf8_lossy(&input)
                } else {
                    let (valid, rest) = input.split_at(error.valid_up_to());
                    self.flush_buffer.extend_from_slice(rest);
                    if valid.is_empty() {
                        return;
                    }

                    // SAFETY: input is valid utf-8
                    unsafe { std::str::from_utf8_unchecked(valid) }.into()
                };

                self.ctx.with_async(|t| t.log(self.group, s)).await;
            }
        }
    }
}

impl HostOutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if !self.ctx.is_null() {
            self.buffer.extend(bytes);
            if self.buffer.len() >= MAX_BUFFER {
                self.flush_buffer.append(&mut self.buffer);
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if self.ctx.is_null() {
            self.buffer.clear();
        } else {
            self.flush_buffer.append(&mut self.buffer);
        }
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if MAX_BUFFER > (self.buffer.len() + self.flush_buffer.len()) {
            Ok(MAX_BUFFER - self.buffer.len() + self.flush_buffer.len())
        } else {
            Ok(0)
        }
    }
}
