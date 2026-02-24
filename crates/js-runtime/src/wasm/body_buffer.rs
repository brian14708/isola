// Body buffer implementations for the JS runtime.
// These are used by the HTTP client to decode response bodies
// into appropriate JS types.
//
// Unlike the Python runtime which uses trait objects,
// the JS HTTP client reads the entire body eagerly and
// provides text()/json() methods on the response object.
// This module provides the lower-level buffer types for
// potential streaming support in the future.

#![allow(dead_code)]

use std::collections::VecDeque;

pub trait BodyBuffer: Default {
    fn write(&mut self, data: Vec<u8>);
    fn close(&mut self);
    fn into_bytes(self) -> Vec<u8>;
}

#[derive(Default)]
pub struct BytesBuffer {
    buffer: Vec<u8>,
}

impl BodyBuffer for BytesBuffer {
    fn write(&mut self, data: Vec<u8>) {
        self.buffer.extend(data);
    }

    fn close(&mut self) {}

    fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }
}

#[derive(Default)]
pub struct LinesBuffer {
    buffer: VecDeque<u8>,
    lines: Vec<String>,
    closed: bool,
}

impl BodyBuffer for LinesBuffer {
    fn write(&mut self, data: Vec<u8>) {
        self.buffer.extend(data);
        self.drain_lines();
    }

    fn close(&mut self) {
        self.closed = true;
        // Flush remaining data as final line
        if !self.buffer.is_empty() {
            let remaining: Vec<u8> = self.buffer.drain(..).collect();
            if let Ok(s) = String::from_utf8(remaining) {
                self.lines.push(s);
            }
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.lines.join("\n").into_bytes()
    }
}

impl LinesBuffer {
    fn drain_lines(&mut self) {
        while let Some(idx) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=idx).collect();
            if let Ok(s) = String::from_utf8(line) {
                self.lines.push(s);
            }
        }
    }
}
