#[pyo3::pymodule]
#[pyo3(name = "_promptkit_http")]
pub mod http_module {
    use std::borrow::Cow;
    use std::io::{BufWriter, Write};

    use pyo3::{
        prelude::*,
        types::{PyBytes, PyDict},
    };
    use url::Url;

    use crate::{
        serde::PyValue,
        wasm::{
            body_buffer::{BodyBuffer, Buffer},
            future::create_future,
            wasi::{
                http::{
                    outgoing_handler::{
                        handle, ErrorCode, FutureIncomingResponse, OutgoingRequest, RequestOptions,
                    },
                    types::{Fields, IncomingBody, IncomingResponse, Method, OutgoingBody, Scheme},
                },
                io::{
                    poll::Pollable,
                    streams::{InputStream, StreamError},
                },
            },
            PyPollable,
        },
    };

    #[pyfunction]
    fn new_buffer(kind: &str) -> ResponseBuffer {
        ResponseBuffer {
            inner: Buffer::new(kind),
        }
    }

    #[pyfunction]
    fn loads_json<'py>(py: Python<'py>, s: &str) -> PyResult<Bound<'py, PyAny>> {
        PyValue::deserialize(py, &mut serde_json::Deserializer::from_str(s))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))
    }

    #[pyfunction]
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
        enum Body<'a> {
            None,
            Bytes(Bound<'a, PyBytes>),
            Object(Bound<'a, PyAny>),
        }

        let body = if let Some(body) = body {
            if let Ok(b) = body.extract::<Bound<PyBytes>>(py) {
                Body::Bytes(b)
            } else {
                Body::Object(body.into_bound(py))
            }
        } else {
            Body::None
        };

        let header_fields = Fields::new();
        if let Some(headers) = headers {
            for (k, v) in headers {
                let k: String = k.extract()?;
                let v: &str = v.extract()?;
                let v = v.as_bytes().to_vec();
                header_fields
                    .append(&k, &v)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            }
        }
        if matches!(body, Body::Object(_)) && !header_fields.has("content-type") {
            header_fields
                .append("content-type", "application/json".as_bytes())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        }

        let request = OutgoingRequest::new(header_fields);
        request
            .set_method(&match method {
                "GET" => Method::Get,
                "POST" => Method::Post,
                "DELETE" => Method::Delete,
                "HEAD" => Method::Head,
                "PATCH" => Method::Patch,
                "PUT" => Method::Put,
                m => Method::Other(m.to_string()),
            })
            .map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid method".to_string())
            })?;
        let mut u = Url::parse(url)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        if let Some(params) = params {
            for (k, v) in params {
                u.query_pairs_mut().append_pair(k.extract()?, v.extract()?);
            }
        }
        request.set_authority(Some(u.authority())).map_err(|()| {
            PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid authority".to_string())
        })?;
        request
            .set_scheme(Some(&match u.scheme() {
                "http" => Scheme::Http,
                "https" => Scheme::Https,
                s => Scheme::Other(s.to_string()),
            }))
            .map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid scheme".to_string())
            })?;
        let mut pq = Cow::Borrowed(u.path());
        if let Some(q) = u.query() {
            let mut copy = pq.to_string();
            copy.push('?');
            copy.push_str(q);
            pq = Cow::Owned(copy);
        }

        request.set_path_with_query(Some(&pq)).map_err(|()| {
            PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid path".to_string())
        })?;

        let opt = RequestOptions::new();
        if let Some(timeout) = timeout {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            opt.set_first_byte_timeout(Some((timeout * 1_000_000_000.0) as u64))
                .map_err(|()| {
                    PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid timeout".to_string())
                })?;
        }

        let ob = if matches!(body, Body::None) {
            None
        } else {
            Some(request.body().map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid body".to_string())
            })?)
        };

        let resp = handle(request, Some(opt))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;

        if let Some(ob) = ob {
            let mut os = ob.write().map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid body".to_string())
            })?;

            match body {
                Body::None => {}
                Body::Bytes(b) => {
                    os.write_all(b.as_bytes()).map_err(|_e| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid body".to_string())
                    })?;
                }
                Body::Object(b) => {
                    serde_json::to_writer(BufWriter::new(&mut os), &PyValue::new(b)).map_err(
                        |_e| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"),
                    )?;
                }
            }

            drop(os);
            OutgoingBody::finish(ob, None).map_err(|_e| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid body".to_string())
            })?;
        }

        Ok(PyFutureResponse::new(resp))
    }

    create_future!(PyFutureResponse, FutureIncomingResponse, PyResponse);

    #[pyclass]
    struct PyResponse {
        response: Option<IncomingResponse>,
        body: Option<IncomingBody>,
        stream: Option<InputStream>,
    }

    impl TryFrom<Result<IncomingResponse, ErrorCode>> for PyResponse {
        type Error = PyErr;

        fn try_from(value: Result<IncomingResponse, ErrorCode>) -> Result<Self, Self::Error> {
            match value {
                Ok(response) => Ok(Self {
                    response: Some(response),
                    body: None,
                    stream: None,
                }),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    e.to_string(),
                )),
            }
        }
    }

    #[pymethods]
    impl PyResponse {
        fn close(&mut self) {
            self.stream.take();
            self.body.take();
            self.response.take();
        }

        fn status(&self) -> u16 {
            self.response.as_ref().expect("response closed").status()
        }

        #[allow(clippy::needless_pass_by_value)]
        fn headers(slf: PyRef<'_, Self>) -> PyResult<Bound<'_, PyDict>> {
            let hdrs = slf.response.as_ref().expect("response closed").headers();
            let d = PyDict::new(slf.py());
            for (k, v) in hdrs.entries() {
                d.set_item(
                    k,
                    std::str::from_utf8(&v).map_err(|_| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid header value")
                    })?,
                )?;
            }
            Ok(d)
        }

        fn read_into(&mut self, buf: &mut ResponseBuffer) -> PyResult<Option<PyPollable>> {
            read_into(self, &mut buf.inner).map(|p| p.map(Into::into))
        }

        fn blocking_read<'py>(
            &mut self,
            py: Python<'py>,
            kind: &str,
        ) -> PyResult<Option<Bound<'py, PyAny>>> {
            let mut buf = Buffer::new(kind);
            while let Some(p) = read_into(self, &mut buf)? {
                p.block();
            }
            buf.decode_all(py)
        }
    }

    impl Drop for PyResponse {
        fn drop(&mut self) {
            self.stream.take();
            self.body.take();
            self.response.take();
        }
    }

    fn read_into(slf: &mut PyResponse, buf: &mut impl BodyBuffer) -> PyResult<Option<Pollable>> {
        let stream = if let Some(b) = slf.stream.as_mut() {
            b
        } else {
            slf.body = Some(
                slf.response
                    .as_mut()
                    .expect("response closed")
                    .consume()
                    .expect("body already read"),
            );
            slf.stream = Some(
                slf.body
                    .as_ref()
                    .unwrap()
                    .stream()
                    .expect("body already read"),
            );
            slf.stream.as_mut().unwrap()
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
                    slf.stream.take();
                    slf.body.take();
                    return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        e.to_debug_string(),
                    ));
                }
                Err(StreamError::Closed) => {
                    buf.close();
                    slf.stream.take();
                    slf.body.take();
                    return Ok(None);
                }
            }
        }
    }

    #[pyclass]
    struct ResponseBuffer {
        inner: Buffer,
    }

    #[pymethods]
    impl ResponseBuffer {
        fn next(&mut self, py: Python<'_>) -> PyResult<Option<PyObject>> {
            self.inner.decode(py).map(|o| o.map(Into::into))
        }

        fn read_all(&mut self, py: Python<'_>) -> PyResult<Option<PyObject>> {
            self.inner.decode_all(py).map(|o| o.map(Into::into))
        }
    }
}
