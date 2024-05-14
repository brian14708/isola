use bytes::Bytes;
use smallvec::SmallVec;
use tracing::event;
use wasmtime_wasi::{HostOutputStream, StdoutStream, StreamResult, Subscribe};

pub struct TraceOutput {
    group: &'static str,
}

impl TraceOutput {
    pub const fn new(group: &'static str) -> Self {
        Self { group }
    }
}

impl StdoutStream for TraceOutput {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(TraceOutputStream {
            group: self.group,
            buffer: SmallVec::new(),
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

pub struct TraceOutputStream {
    group: &'static str,
    buffer: SmallVec<[u8; MAX_BUFFER + MAX_UTF8_BYTES]>,
}

const MIN_BUFFER: usize = 64;
const MAX_BUFFER: usize = 1024;
const MAX_UTF8_BYTES: usize = 4;

impl TraceOutputStream {
    fn record(&self, s: &str) {
        match self.group {
            "stderr" => {
                event!(
                    target: "promptkit::stderr",
                    tracing::Level::DEBUG,
                    promptkit.user = true,
                    promptkit.log.group = "stderr",
                    promptkit.log.output = s,
                );
            }
            "stdout" => {
                event!(
                    target: "promptkit::stdout",
                    tracing::Level::DEBUG,
                    promptkit.user = true,
                    promptkit.log.group = "stdout",
                    promptkit.log.output = s,
                );
            }
            v => {
                event!(
                    target: "promptkit::log",
                    tracing::Level::DEBUG,
                    promptkit.user = true,
                    promptkit.log.group = v,
                    promptkit.log.output = s,
                );
            }
        }
    }
}

#[async_trait::async_trait]
impl Subscribe for TraceOutputStream {
    async fn ready(&mut self) {}
}

impl HostOutputStream for TraceOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if bytes.len() + self.buffer.len() < MIN_BUFFER {
            self.buffer.extend_from_slice(&bytes);
            return Ok(());
        }

        let (s, v) = {
            let buf: &[u8] = if self.buffer.is_empty() {
                &bytes
            } else {
                self.buffer.extend(bytes);
                &self.buffer
            };
            match std::str::from_utf8(buf) {
                Ok(s) => (s.into(), SmallVec::new_const()),
                Err(error) => {
                    if buf.len() - error.valid_up_to() > MAX_UTF8_BYTES {
                        // not a valid utf-8 sequence
                        (String::from_utf8_lossy(buf), SmallVec::new_const())
                    } else {
                        let (valid, rest) = buf.split_at(error.valid_up_to());
                        if valid.is_empty() {
                            return Ok(());
                        }

                        (
                            // SAFETY: input is valid utf-8
                            unsafe { std::str::from_utf8_unchecked(valid) }.into(),
                            (SmallVec::<[u8; MAX_UTF8_BYTES]>::from_slice(rest)),
                        )
                    }
                }
            }
        };
        self.record(&s);
        self.buffer.clear();
        if !v.is_empty() {
            self.buffer.extend_from_slice(&v);
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let buf = &self.buffer;
        let (s, v) = match std::str::from_utf8(buf) {
            Ok(s) => (s.into(), SmallVec::new_const()),
            Err(error) => {
                if buf.len() - error.valid_up_to() > MAX_UTF8_BYTES {
                    // not a valid utf-8 sequence
                    (String::from_utf8_lossy(buf), SmallVec::new_const())
                } else {
                    let (valid, rest) = buf.split_at(error.valid_up_to());
                    if valid.is_empty() {
                        return Ok(());
                    }

                    (
                        // SAFETY: input is valid utf-8
                        unsafe { std::str::from_utf8_unchecked(valid) }.into(),
                        (SmallVec::<[u8; MAX_UTF8_BYTES]>::from_slice(rest)),
                    )
                }
            }
        };
        self.record(&s);
        self.buffer.clear();
        if !v.is_empty() {
            self.buffer.extend_from_slice(&v);
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
