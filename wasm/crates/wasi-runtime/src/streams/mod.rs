use std::{io::Read, pin};

use futures::Future;
use wasi::io::streams::{InputStream, StreamError};

use crate::futures::Reactor;

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

pub struct AsyncStreamReader<'r> {
    stream: InputStream,
    reactor: &'r Reactor,
}

impl<'r> AsyncStreamReader<'r> {
    pub fn new(stream: InputStream, reactor: &'r Reactor) -> Self {
        Self { stream, reactor }
    }
}

impl<'r> futures::io::AsyncRead for AsyncStreamReader<'r> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let fut = self.reactor.wait_for(self.stream.subscribe());
        let fut = pin::pin!(fut);
        fut.poll(cx)
            .map(|_| match self.stream.read(buf.len() as u64) {
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
            })
    }
}
