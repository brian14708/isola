use std::{
    borrow::Cow,
    future::Future,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::task::noop_waker_ref;
use parking_lot::Mutex;
use smallvec::SmallVec;
use tokio::io::AsyncWrite;
use wasmtime_wasi::{
    cli::{IsTerminal, StdoutStream},
    p2::{OutputStream, Pollable, StreamError, StreamResult},
};

use crate::host::{LogContext, LogLevel, OutputSink};

pub struct TraceOutput {
    level: LogLevel,
    context: LogContext<'static>,
    sink_store: LogSinkStore,
}

impl TraceOutput {
    pub const fn new(
        level: LogLevel,
        context: LogContext<'static>,
        sink_store: LogSinkStore,
    ) -> Self {
        Self {
            level,
            context,
            sink_store,
        }
    }
}

impl StdoutStream for TraceOutput {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        // Preview2 uses `p2_stream` for stdout/stderr; this is a best-effort sink.
        Box::new(tokio::io::sink())
    }

    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(TraceOutputStream {
            level: self.level,
            context: self.context,
            sink_store: Arc::clone(&self.sink_store),
            buffer: SmallVec::new(),
            in_flight: None,
            last_error: None,
        })
    }
}

impl IsTerminal for TraceOutput {
    fn is_terminal(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    level: LogLevel,
    context: LogContext<'static>,
    sink_store: LogSinkStore,
    buffer: SmallVec<[u8; MAX_BUFFER + MAX_UTF8_BYTES]>,
    in_flight: Option<wasmtime_wasi::runtime::AbortOnDropJoinHandle<anyhow::Result<()>>>,
    last_error: Option<anyhow::Error>,
}

const MIN_BUFFER: usize = 64;
const MAX_BUFFER: usize = 1024;
const MAX_UTF8_BYTES: usize = 4;

impl TraceOutputStream {
    fn record(&mut self, s: &str) -> StreamResult<()> {
        if self.in_flight.is_some() {
            return Err(StreamError::Trap(anyhow::anyhow!(
                "write not permitted while emit pending"
            )));
        }

        if s.is_empty() {
            return Ok(());
        }

        let Some(sink) = self.sink_store.lock().clone() else {
            return Ok(());
        };

        let level = self.level;
        let context = self.context;
        let message = s.to_string();
        let mut future = Box::pin(async move {
            sink.on_log(level, context, &message)
                .await
                .map_err(anyhow::Error::from_boxed)
        });
        let waker = noop_waker_ref();
        let mut cx = Context::from_waker(waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(result) => result.map_err(StreamError::LastOperationFailed),
            Poll::Pending => {
                self.in_flight = Some(wasmtime_wasi::runtime::spawn(future));
                Ok(())
            }
        }
    }
}

#[async_trait::async_trait]
impl Pollable for TraceOutputStream {
    async fn ready(&mut self) {
        if let Some(task) = self.in_flight.take()
            && let Err(error) = task.await
        {
            self.last_error = Some(error);
        }
    }
}

/// Decode as much valid UTF-8 as possible, returning the decoded string and any
/// trailing partial multi-byte bytes that should be retained in the buffer.
fn decode_utf8(buf: &[u8]) -> (Cow<'_, str>, SmallVec<[u8; MAX_UTF8_BYTES]>) {
    match std::str::from_utf8(buf) {
        Ok(s) => (s.into(), SmallVec::new_const()),
        Err(error) => {
            if buf.len() - error.valid_up_to() > MAX_UTF8_BYTES {
                // Not a valid utf-8 sequence; fall back to lossy.
                (String::from_utf8_lossy(buf), SmallVec::new_const())
            } else {
                let (valid, rest) = buf.split_at(error.valid_up_to());
                (
                    if valid.is_empty() {
                        Cow::Borrowed("")
                    } else {
                        // SAFETY: `valid` contains only bytes up to the first
                        // encoding error, which `from_utf8` guarantees is valid.
                        unsafe { std::str::from_utf8_unchecked(valid) }.into()
                    },
                    SmallVec::from_slice(rest),
                )
            }
        }
    }
}

impl OutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if let Some(error) = self.last_error.take() {
            return Err(StreamError::LastOperationFailed(error));
        }

        if self.in_flight.is_some() {
            return Err(StreamError::Trap(anyhow::anyhow!(
                "write not permitted while emit pending"
            )));
        }

        if bytes.len() + self.buffer.len() < MIN_BUFFER {
            self.buffer.extend(bytes);
            return Ok(());
        }

        let buf: &[u8] = if self.buffer.is_empty() {
            &bytes
        } else {
            self.buffer.extend(bytes);
            &self.buffer
        };
        let (s, remainder) = decode_utf8(buf);
        let message = if s.is_empty() {
            None
        } else {
            Some(s.into_owned())
        };
        if let Some(message) = message {
            self.record(&message)?;
        }
        self.buffer.clear();
        if !remainder.is_empty() {
            self.buffer.extend(remainder);
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if let Some(error) = self.last_error.take() {
            return Err(StreamError::LastOperationFailed(error));
        }

        // `flush` can be called immediately after `write`; don't trap while a
        // previous emit is still in flight. Backpressure is enforced via
        // `check_write` returning 0 until `ready` observes completion.
        if self.in_flight.is_some() {
            return Ok(());
        }

        if !self.buffer.is_empty() {
            let (s, remainder) = decode_utf8(&self.buffer);
            let message = if s.is_empty() {
                None
            } else {
                Some(s.into_owned())
            };
            if let Some(message) = message {
                self.record(&message)?;
            }
            self.buffer.clear();
            if !remainder.is_empty() {
                self.buffer.extend(remainder);
            }
        }
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if let Some(error) = self.last_error.take() {
            return Err(StreamError::LastOperationFailed(error));
        }

        if self.in_flight.is_some() {
            return Ok(0);
        }

        let local_capacity = MAX_BUFFER.saturating_sub(self.buffer.len());
        Ok(local_capacity)
    }
}

pub type LogSinkStore = Arc<Mutex<Option<Arc<dyn OutputSink>>>>;

#[must_use]
pub fn new_log_sink_store() -> LogSinkStore {
    Arc::new(Mutex::new(None))
}

pub fn set_log_sink(store: &LogSinkStore, sink: Option<Arc<dyn OutputSink>>) {
    let mut guard = store.lock();
    *guard = sink;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_stream() -> TraceOutputStream {
        TraceOutputStream {
            level: LogLevel::Info,
            context: LogContext::Other("test"),
            sink_store: new_log_sink_store(),
            buffer: SmallVec::new(),
            in_flight: None,
            last_error: None,
        }
    }

    #[test]
    fn small_write_buffers() {
        let mut s = new_stream();
        s.write(Bytes::from_static(b"hi")).unwrap();
        assert_eq!(s.buffer.as_slice(), b"hi");
    }

    #[test]
    fn large_write_flushes_buffer() {
        let mut s = new_stream();
        let data = Bytes::from(vec![b'a'; MIN_BUFFER + 1]);
        s.write(data).unwrap();
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn partial_utf8_retained() {
        let mut s = new_stream();
        // Valid ASCII prefix + first byte of a 2-byte UTF-8 char (U+00FC = 0xC3 0xBC)
        let mut data = vec![b'a'; MIN_BUFFER];
        data.push(0xC3);
        s.write(Bytes::from(data)).unwrap();
        // The partial byte should be retained in the buffer
        assert_eq!(s.buffer.as_slice(), &[0xC3]);
    }

    #[test]
    fn partial_utf8_completed_on_next_write() {
        let mut s = new_stream();
        let mut data = vec![b'a'; MIN_BUFFER];
        data.push(0xC3); // first byte of ü
        s.write(Bytes::from(data)).unwrap();
        assert_eq!(s.buffer.as_slice(), &[0xC3]);

        // Complete the multi-byte char + more data to trigger flush
        let mut data2 = vec![0xBC]; // second byte of ü
        data2.extend(vec![b'b'; MIN_BUFFER]);
        s.write(Bytes::from(data2)).unwrap();
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn flush_emits_buffered() {
        let mut s = new_stream();
        s.write(Bytes::from_static(b"hi")).unwrap();
        assert!(!s.buffer.is_empty());
        s.flush().unwrap();
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn flush_noop_when_empty() {
        let mut s = new_stream();
        s.flush().unwrap();
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn check_write_capacity() {
        let mut s = new_stream();
        assert_eq!(s.check_write().unwrap(), MAX_BUFFER);

        s.buffer.extend(vec![b'x'; MAX_BUFFER]);
        assert_eq!(s.check_write().unwrap(), 0);
    }

    #[test]
    fn invalid_utf8_uses_lossy() {
        let mut s = new_stream();
        // Invalid UTF-8: continuation bytes without start byte, enough to exceed
        // MAX_UTF8_BYTES
        let data = vec![0xFF; MAX_UTF8_BYTES + MIN_BUFFER + 1];
        s.write(Bytes::from(data)).unwrap();
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn flush_with_partial_utf8_retains_tail() {
        let mut s = new_stream();
        // Buffer some valid ASCII + partial multi-byte
        s.buffer.extend_from_slice(b"hello");
        s.buffer.push(0xE2); // first byte of a 3-byte char
        s.flush().unwrap();
        // The partial byte should remain
        assert_eq!(s.buffer.as_slice(), &[0xE2]);
    }
}
