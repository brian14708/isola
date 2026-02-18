use std::borrow::Cow;

use crate::TRACE_TARGET_SCRIPT;
use bytes::Bytes;
use smallvec::SmallVec;
use tokio::io::AsyncWrite;
use tracing::event;
use wasmtime_wasi::{
    cli::{IsTerminal, StdoutStream},
    p2::{OutputStream, Pollable, StreamResult},
};

pub struct TraceOutput {
    context: &'static str,
}

impl TraceOutput {
    pub const fn new(context: &'static str) -> Self {
        Self { context }
    }
}

impl StdoutStream for TraceOutput {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        // Preview2 uses `p2_stream` for stdout/stderr; this is a best-effort sink.
        Box::new(tokio::io::sink())
    }

    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(TraceOutputStream {
            context: self.context,
            buffer: SmallVec::new(),
        })
    }
}

impl IsTerminal for TraceOutput {
    fn is_terminal(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    context: &'static str,
    buffer: SmallVec<[u8; MAX_BUFFER + MAX_UTF8_BYTES]>,
}

const MIN_BUFFER: usize = 64;
const MAX_BUFFER: usize = 1024;
const MAX_UTF8_BYTES: usize = 4;

impl TraceOutputStream {
    fn record(&self, s: &str) {
        event!(
            name: "log",
            target: TRACE_TARGET_SCRIPT,
            tracing::Level::DEBUG,
            log.context = self.context,
            log.output = s,
        );
    }
}

#[async_trait::async_trait]
impl Pollable for TraceOutputStream {
    async fn ready(&mut self) {}
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
        if !s.is_empty() {
            self.record(&s);
        }
        self.buffer.clear();
        if !remainder.is_empty() {
            self.buffer.extend(remainder);
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let (s, remainder) = decode_utf8(&self.buffer);
        if !s.is_empty() {
            self.record(&s);
        }
        self.buffer.clear();
        if !remainder.is_empty() {
            self.buffer.extend(remainder);
        }
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        if MAX_BUFFER > self.buffer.len() {
            Ok(MAX_BUFFER - self.buffer.len())
        } else {
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_stream() -> TraceOutputStream {
        TraceOutputStream {
            context: "test",
            buffer: SmallVec::new(),
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
        // Invalid UTF-8: continuation bytes without start byte, enough to exceed MAX_UTF8_BYTES
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
