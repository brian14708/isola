use core::panic;
use std::io::Write;

use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyBytes, PyDict},
};
use serde::de::DeserializeSeed;
use url::Url;
use wasi::io::{
    poll::Pollable,
    streams::{InputStream, StreamError},
};

use crate::{
    serde::{PyObjectDeserializer, PyObjectSerializer},
    wasm::{
        body_buffer,
        promptkit::http::client::{self, Method},
    },
};

use super::{
    body_buffer::BodyBuffer,
    future::create_future,
    promptkit::http::client::{FutureResponse, HttpError, Request, Response},
    PyPollable,
};

#[pymodule]
#[pyo3(name = "_promptkit_http")]
pub fn http_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyfn(module)]
    fn new_buffer(kind: &str) -> Buffer {
        Buffer {
            inner: body_buffer::Buffer::new(kind),
        }
    }

    #[pyfn(module)]
    fn loads_json<'py>(py: Python<'py>, s: &str) -> PyResult<Bound<'py, PyAny>> {
        Ok(PyObjectDeserializer::new(py)
            .deserialize(&mut serde_json::Deserializer::from_str(s))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?
            .into_bound(py))
    }

    #[pyfn(module)]
    #[pyo3(signature = (method, url, params, headers, body, timeout))]
    fn fetch(
        py: Python<'_>,
        method: &str,
        url: &str,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        body: Option<PyObject>,
        timeout: Option<f64>,
    ) -> PyResult<PyFutureResponse> {
        let mut request = client::Request::new(match method {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "DELETE" => Method::Delete,
            "HEAD" => Method::Head,
            "PATCH" => Method::Patch,
            "PUT" => Method::Put,
            _ => {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "invalid method".to_string(),
                ))
            }
        });

        if let Some(params) = params {
            let mut u = Url::parse(url)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            for (k, v) in params {
                u.query_pairs_mut().append_pair(k.extract()?, v.extract()?);
            }
            request.set_url(u.as_ref())
        } else {
            request.set_url(url)
        }
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;

        if let Some(headers) = headers {
            for (k, v) in headers {
                request
                    .set_header(k.extract()?, v.extract()?)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            }
        }

        if let Some(timeout) = timeout {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            request.set_timeout((timeout * 1_000_000_000.0) as u64);
        }

        if let Some(body) = body {
            if let Ok(b) = body.extract::<Bound<PyBytes>>(py) {
                request
                    .write_body(b.as_bytes())
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            } else {
                struct RequestWriter<'a>(&'a mut Request);
                impl Write for RequestWriter<'_> {
                    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                        self.0
                            .write_body(buf)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        Ok(buf.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }

                request
                    .set_header("content-type", "application/json")
                    .unwrap();
                PyObjectSerializer::to_json_writer(
                    RequestWriter(&mut request),
                    body.into_bound(py),
                )
                .map_err(|_e| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))?;
            }
        }

        let request = client::fetch(request)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        Ok(PyFutureResponse::new(request))
    }

    Ok(())
}

create_future!(PyFutureResponse, FutureResponse, PyResponse);

#[pyclass]
struct PyResponse {
    response: Option<Response>,
    body: Option<InputStream>,
}

impl TryFrom<Result<Response, HttpError>> for PyResponse {
    type Error = PyErr;

    fn try_from(value: Result<Response, HttpError>) -> Result<Self, Self::Error> {
        match value {
            Ok(response) => Ok(Self {
                response: Some(response),
                body: None,
            }),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(match e {
                HttpError::Cancelled => "cancelled".to_string(),
                HttpError::Timeout => "timeout".to_string(),
                HttpError::StatusCode(code) => {
                    format!("status code: {code}")
                }
                HttpError::Unknown(s) => s,
            })),
        }
    }
}

#[pymethods]
impl PyResponse {
    fn close(&mut self) {
        self.body.take();
        self.response.take();
    }

    fn status(&self) -> u16 {
        self.response.as_ref().expect("response closed").status()
    }

    #[allow(clippy::needless_pass_by_value)]
    fn headers(slf: PyRef<'_, Self>) -> Bound<'_, PyDict> {
        slf.response
            .as_ref()
            .expect("response closed")
            .headers()
            .into_py_dict_bound(slf.py())
    }

    fn read_into(&mut self, buf: &mut Buffer) -> PyResult<Option<PyPollable>> {
        read_into(self, &mut buf.inner).map(|p| p.map(Into::into))
    }

    fn blocking_read<'py>(
        &mut self,
        py: Python<'py>,
        kind: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let mut buf = body_buffer::Buffer::new(kind);
        while let Some(p) = read_into(self, &mut buf)? {
            p.block();
        }
        buf.decode_all(py)
    }
}

fn read_into(
    slf: &mut PyResponse,
    buf: &mut impl body_buffer::BodyBuffer,
) -> PyResult<Option<Pollable>> {
    let stream = if let Some(b) = slf.body.as_mut() {
        b
    } else {
        slf.body = Some(
            slf.response
                .as_mut()
                .expect("response closed")
                .body()
                .expect("body already read"),
        );
        slf.body.as_mut().unwrap()
    };

    loop {
        match InputStream::read(stream, 16384) {
            Ok(v) => {
                if !v.is_empty() {
                    buf.write(v);
                    continue;
                }

                let poll = stream.subscribe();
                return Ok(Some(poll));
            }
            Err(StreamError::LastOperationFailed(e)) => {
                slf.body.take();
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    e.to_debug_string(),
                ));
            }
            Err(StreamError::Closed) => {
                buf.close();
                return Ok(None);
            }
        }
    }
}

#[pyclass]
struct Buffer {
    inner: body_buffer::Buffer,
}

#[pymethods]
impl Buffer {
    fn next(&mut self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        self.inner.decode(py).map(|o| o.map(Into::into))
    }

    fn read_all(&mut self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        self.inner.decode_all(py).map(|o| o.map(Into::into))
    }
}
