use std::{
    borrow::Cow,
    future::{Future, poll_fn},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::task::noop_waker_ref;
use parking_lot::Mutex;
use smallvec::SmallVec;
use tokio::io::AsyncWrite;
use tracing::{Instrument as _, instrument::WithSubscriber as _};
use wasmtime_wasi::{
    cli::{IsTerminal, StdoutStream},
    p2::{OutputStream, Pollable, StreamError, StreamResult},
};

use crate::host::{LogContext, LogLevel, OutputTarget};

pub struct TraceOutput {
    level: LogLevel,
    context: LogContext<'static>,
    target_store: LogTargetStore,
}

impl TraceOutput {
    pub const fn new(
        level: LogLevel,
        context: LogContext<'static>,
        target_store: LogTargetStore,
    ) -> Self {
        Self {
            level,
            context,
            target_store,
        }
    }
}

impl TraceOutput {
    fn stream(&self) -> SharedOutputStream {
        let stream = Arc::new(Mutex::new(TraceOutputStream {
            level: self.level,
            context: self.context,
            target_store: Arc::clone(&self.target_store.target),
            buffer: SmallVec::new(),
            in_flight: None,
            last_error: None,
        }));
        self.target_store.streams.lock().push(Arc::clone(&stream));
        SharedOutputStream(stream)
    }
}

impl StdoutStream for TraceOutput {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.stream())
    }

    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.stream())
    }
}

/// Both WASI interfaces share the same buffering and delivery rules. The
/// registry keeps dropped guest streams alive until call cleanup drains them.
struct SharedOutputStream(Arc<Mutex<TraceOutputStream>>);

#[async_trait::async_trait]
impl Pollable for SharedOutputStream {
    async fn ready(&mut self) {
        poll_fn(|cx| self.0.lock().poll_ready(cx)).await;
    }
}

impl OutputStream for SharedOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.0.lock().write(bytes)
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.0.lock().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.0.lock().check_write()
    }
}

fn io_error(error: &StreamError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

impl AsyncWrite for SharedOutputStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut stream = self.0.lock();
        std::task::ready!(stream.poll_ready(cx));
        let capacity = stream.check_write().map_err(|e| io_error(&e))?;
        let len = capacity.min(bytes.len());
        stream
            .write(Bytes::copy_from_slice(&bytes[..len]))
            .map_err(|e| io_error(&e))?;
        drop(stream);
        Poll::Ready(Ok(len))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut stream = self.0.lock();
        std::task::ready!(stream.poll_ready(cx));
        stream.flush().map_err(|e| io_error(&e))?;
        std::task::ready!(stream.poll_ready(cx));
        stream.check_write().map_err(|e| io_error(&e))?;
        drop(stream);
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.0.lock().poll_finish(cx).map_err(|e| io_error(&e))
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
    target_store: Arc<Mutex<Option<OutputTarget>>>,
    buffer: SmallVec<[u8; MAX_BUFFER + MAX_UTF8_BYTES]>,
    in_flight: Option<wasmtime_wasi::runtime::AbortOnDropJoinHandle<wasmtime::Result<()>>>,
    last_error: Option<wasmtime::Error>,
}

const MIN_BUFFER: usize = 64;
const MAX_BUFFER: usize = 1024;
const MAX_UTF8_BYTES: usize = 4;

impl TraceOutputStream {
    fn record(&mut self, message: String) -> StreamResult<()> {
        if self.in_flight.is_some() {
            return Err(StreamError::Trap(wasmtime::Error::msg(
                "write not permitted while emit pending",
            )));
        }

        if message.is_empty() {
            return Ok(());
        }

        let Some(target) = self.target_store.lock().clone() else {
            return Ok(());
        };

        let level = self.level;
        let context = self.context;
        let mut future = Box::pin(async move {
            target
                .on_log(level, context, &message)
                .await
                .map_err(wasmtime::Error::from_boxed)
        });
        let waker = noop_waker_ref();
        let mut cx = Context::from_waker(waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(result) => result.map_err(StreamError::LastOperationFailed),
            Poll::Pending => {
                self.in_flight = Some(wasmtime_wasi::runtime::spawn(
                    future.in_current_span().with_current_subscriber(),
                ));
                Ok(())
            }
        }
    }
}

impl TraceOutputStream {
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if let Some(task) = self.in_flight.as_mut() {
            let result = std::task::ready!(Pin::new(task).poll(cx));
            self.in_flight = None;
            if let Err(error) = result {
                self.last_error = Some(error);
            }
        }
        Poll::Ready(())
    }

    fn poll_finish(&mut self, cx: &mut Context<'_>) -> Poll<StreamResult<()>> {
        std::task::ready!(self.poll_ready(cx));
        self.check_write()?;
        // At an operation boundary even incomplete UTF-8 must be emitted;
        // retaining it would mix bytes from different calls.
        if !self.buffer.is_empty() {
            let message = String::from_utf8_lossy(&self.buffer).into_owned();
            self.buffer.clear();
            self.record(message)?;
        }
        std::task::ready!(self.poll_ready(cx));
        self.check_write()?;
        Poll::Ready(Ok(()))
    }
}

#[async_trait::async_trait]
impl Pollable for TraceOutputStream {
    async fn ready(&mut self) {
        poll_fn(|cx| self.poll_ready(cx)).await;
    }
}

/// Replace invalid sequences, retaining only an actually incomplete trailing
/// sequence so a character split across writes can be reassembled.
fn decode_utf8(buf: &[u8]) -> (Cow<'_, str>, SmallVec<[u8; MAX_UTF8_BYTES]>) {
    let mut offset = 0;
    while let Err(error) = std::str::from_utf8(&buf[offset..]) {
        offset += error.valid_up_to();
        if let Some(len) = error.error_len() {
            offset += len;
        } else {
            return (
                String::from_utf8_lossy(&buf[..offset]),
                SmallVec::from_slice(&buf[offset..]),
            );
        }
    }
    (String::from_utf8_lossy(buf), SmallVec::new_const())
}

impl OutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if let Some(error) = self.last_error.take() {
            return Err(StreamError::LastOperationFailed(error));
        }

        if self.in_flight.is_some() {
            return Err(StreamError::Trap(wasmtime::Error::msg(
                "write not permitted while emit pending",
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
        if !s.is_empty() {
            self.record(s.into_owned())?;
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
            if !s.is_empty() {
                self.record(s.into_owned())?;
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

#[derive(Clone, Default)]
pub struct LogTargetStore {
    target: Arc<Mutex<Option<OutputTarget>>>,
    streams: Arc<Mutex<Vec<Arc<Mutex<TraceOutputStream>>>>>,
}

#[must_use]
pub fn new_log_target_store() -> LogTargetStore {
    LogTargetStore::default()
}

pub fn set_log_target(store: &LogTargetStore, target: Option<OutputTarget>) {
    // Abort outstanding deliveries and clear bytes/errors even when a call
    // was cancelled before its explicit flush completed.
    let mut streams = store.streams.lock();
    for stream in streams.iter() {
        let mut stream = stream.lock();
        stream.in_flight = None;
        stream.last_error = None;
        stream.buffer.clear();
    }
    streams.retain(|stream| Arc::strong_count(stream) > 1);
    drop(streams);
    *store.target.lock() = target;
}

pub async fn flush_logs(store: &LogTargetStore) -> wasmtime::Result<()> {
    let streams = store.streams.lock().clone();
    for stream in streams {
        poll_fn(|cx| stream.lock().poll_finish(cx))
            .await
            .map_err(wasmtime::Error::new)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_stream() -> TraceOutputStream {
        TraceOutputStream {
            level: LogLevel::Info,
            context: LogContext::Other("test"),
            target_store: new_log_target_store().target,
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
        // Valid ASCII prefix + first byte of a 2-byte UTF-8 char (U+00FC = 0xC3
        // 0xBC)
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
        // Invalid UTF-8: continuation bytes without start byte, enough to
        // exceed MAX_UTF8_BYTES
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

    #[tokio::test]
    async fn preview3_stdout_reaches_target() {
        use tokio::io::AsyncWriteExt as _;

        let store = new_log_target_store();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        set_log_target(&store, Some(OutputTarget::unbounded(tx)));
        let output = TraceOutput::new(LogLevel::Stdout, LogContext::Stdout, store);
        let mut writer = Box::into_pin(output.async_stream());
        writer.as_mut().write_all(b"preview3").await.unwrap();
        writer.as_mut().flush().await.unwrap();
        let crate::host::OutputEvent::Log { message, .. } = rx.try_recv().unwrap() else {
            panic!("expected stdout");
        };
        assert_eq!(message, "preview3");
    }
}
