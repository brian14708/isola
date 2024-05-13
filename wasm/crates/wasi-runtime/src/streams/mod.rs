use std::io::Read;

use wasi::io::streams::{InputStream, StreamError};

pub struct BlockingStreamReader(InputStream);

impl BlockingStreamReader {
    pub fn new(stream: InputStream) -> Self {
        Self(stream)
    }
}

impl Read for BlockingStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.0.blocking_read(buf.len() as u64) {
            Ok(output) => {
                let len = output.len();
                buf[..len].copy_from_slice(&output);
                Ok(len)
            }
            Err(StreamError::Closed) => Ok(0),
            Err(StreamError::LastOperationFailed(e)) => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_debug_string(),
            )),
        }
    }
}
