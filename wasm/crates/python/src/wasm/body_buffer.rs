use std::{collections::VecDeque, io::Read};

use eventsource::event::parse_event_line;
use pyo3::{
    types::{PyBytes, PyList, PyString},
    Bound, PyAny, PyErr, PyResult, Python, ToPyObject,
};
use serde::de::DeserializeSeed;

use crate::serde::PyObjectDeserializer;

pub trait BodyBuffer: Default {
    fn write(&mut self, data: Vec<u8>);
    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>>;
    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>>;
    fn close(&mut self);
}

pub enum Buffer {
    Bytes(Bytes),
    Lines(Lines),
    Json(Json),
    Text(Text),
    ServerSentEvent(ServerSentEvent),
}

impl Buffer {
    pub fn new(kind: &str) -> Self {
        match kind {
            "binary" | "bytes" => Self::Bytes(Bytes::default()),
            "lines" => Self::Lines(Lines::default()),
            "json" => Self::Json(Json::default()),
            "text" => Self::Text(Text::default()),
            "sse" => Self::ServerSentEvent(ServerSentEvent::default()),
            _ => panic!("Invalid buffer kind: {kind}"),
        }
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::Bytes(Bytes::default())
    }
}

impl BodyBuffer for Buffer {
    fn write(&mut self, data: Vec<u8>) {
        match self {
            Self::Bytes(b) => b.write(data),
            Self::Lines(b) => b.write(data),
            Self::Json(b) => b.write(data),
            Self::Text(b) => b.write(data),
            Self::ServerSentEvent(b) => b.write(data),
        }
    }

    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        match self {
            Self::Bytes(b) => b.decode(py),
            Self::Lines(b) => b.decode(py),
            Self::Json(b) => b.decode(py),
            Self::Text(b) => b.decode(py),
            Self::ServerSentEvent(b) => b.decode(py),
        }
    }

    fn close(&mut self) {
        match self {
            Self::Bytes(b) => b.close(),
            Self::Lines(b) => b.close(),
            Self::Json(b) => b.close(),
            Self::Text(b) => b.close(),
            Self::ServerSentEvent(b) => b.close(),
        }
    }

    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        match self {
            Self::Bytes(b) => b.decode_all(py),
            Self::Lines(b) => b.decode_all(py),
            Self::Json(b) => b.decode_all(py),
            Self::Text(b) => b.decode_all(py),
            Self::ServerSentEvent(b) => b.decode_all(py),
        }
    }
}

#[derive(Default)]
pub struct Bytes {
    buffer: Vec<u8>,
}

impl BodyBuffer for Bytes {
    fn write(&mut self, data: Vec<u8>) {
        if self.buffer.is_empty() {
            self.buffer = data;
        } else {
            self.buffer.extend(data);
        }
    }

    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            PyBytes::new_bound_with(py, self.buffer.len(), |dest| {
                dest.copy_from_slice(&std::mem::take(&mut self.buffer));
                Ok(())
            })
            .unwrap()
            .into_any(),
        ))
    }

    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.decode(py)
    }

    fn close(&mut self) {}
}

#[derive(Default)]
pub struct Lines {
    buffer: VecDeque<u8>,
    closed: bool,
}

impl BodyBuffer for Lines {
    fn write(&mut self, data: Vec<u8>) {
        if self.buffer.is_empty() {
            self.buffer = data.into();
        } else {
            self.buffer.extend(data);
        }
    }

    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        let b = match (self.closed, self.buffer.iter().position(|&b| b == b'\n')) {
            (_, Some(idx)) => {
                let mut b = vec![0; idx + 1];
                self.buffer.read_exact(&mut b).unwrap();
                b
            }
            (true, None) => {
                if self.buffer.is_empty() {
                    return Ok(None);
                }

                std::mem::take(&mut self.buffer).into()
            }
            (false, None) => return Ok(None),
        };

        Ok(Some(
            PyString::new_bound(
                py,
                &String::from_utf8(b)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?,
            )
            .into_any(),
        ))
    }

    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        if self.buffer.is_empty() {
            return Ok(None);
        }

        let mut results = Vec::new();
        while let Some(obj) = self.decode(py)? {
            results.push(obj);
        }
        Ok(Some(PyList::new_bound(py, results).into_any()))
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

#[derive(Default)]
pub struct Json {
    buffer: Vec<u8>,
    closed: bool,
}

impl BodyBuffer for Json {
    fn write(&mut self, data: Vec<u8>) {
        if self.buffer.is_empty() {
            self.buffer = data;
        } else {
            self.buffer.extend(data);
        }
    }

    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        if !self.closed {
            return Ok(None);
        }
        if self.buffer.is_empty() {
            return Ok(None);
        }

        let obj = PyObjectDeserializer::new(py)
            .deserialize(&mut serde_json::Deserializer::from_reader(
                std::io::Cursor::new(std::mem::take(&mut self.buffer)),
            ))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?
            .into_bound(py);
        Ok(Some(obj))
    }

    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.decode(py)
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

#[derive(Default)]
pub struct Text {
    buffer: Vec<u8>,
    closed: bool,
}

impl BodyBuffer for Text {
    fn write(&mut self, data: Vec<u8>) {
        if self.buffer.is_empty() {
            self.buffer = data;
        } else {
            self.buffer.extend(data);
        }
    }

    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        if !self.closed {
            return Ok(None);
        }
        if self.buffer.is_empty() {
            return Ok(None);
        }

        Ok(Some(
            PyString::new_bound(
                py,
                &String::from_utf8(std::mem::take(&mut self.buffer))
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?,
            )
            .into_any(),
        ))
    }

    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.decode(py)
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

pub struct ServerSentEvent {
    buffer: VecDeque<u8>,
    closed: bool,
    event: eventsource::event::Event,
}

impl Default for ServerSentEvent {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
            closed: false,
            event: eventsource::event::Event::new(),
        }
    }
}

impl BodyBuffer for ServerSentEvent {
    fn write(&mut self, data: Vec<u8>) {
        if self.buffer.is_empty() {
            self.buffer = data.into();
        } else {
            self.buffer.extend(data);
        }
    }

    fn decode<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        loop {
            let line = match (self.closed, self.buffer.iter().position(|&b| b == b'\n')) {
                (_, Some(idx)) => {
                    let mut b = vec![0; idx + 1];
                    self.buffer.read_exact(&mut b).unwrap();
                    b
                }
                (true, None) => {
                    if self.buffer.is_empty() {
                        if !self.event.is_empty() {
                            let evt = (&self.event.id, &self.event.event_type, &self.event.data)
                                .to_object(py)
                                .into_bound(py);
                            self.event.clear();
                            return Ok(Some(evt));
                        }
                        return Ok(None);
                    }

                    let s = std::mem::take(&mut self.buffer);
                    s.into()
                }
                (false, None) => return Ok(None),
            };
            let line = String::from_utf8(line)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;

            match parse_event_line(&line, &mut self.event) {
                eventsource::event::ParseResult::Next
                | eventsource::event::ParseResult::SetRetry(_) => continue,
                eventsource::event::ParseResult::Dispatch => {
                    let evt = (&self.event.id, &self.event.event_type, &self.event.data)
                        .to_object(py)
                        .into_bound(py);
                    self.event.clear();
                    return Ok(Some(evt));
                }
            }
        }
    }

    fn decode_all<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        if self.buffer.is_empty() {
            return Ok(None);
        }

        let mut results = Vec::new();
        while let Some(obj) = self.decode(py)? {
            results.push(obj);
        }
        Ok(Some(PyList::new_bound(py, results).into_any()))
    }

    fn close(&mut self) {
        self.closed = true;
    }
}
