#[pyo3::pymodule]
#[pyo3(name = "_isola_http")]
pub mod http_module {
    use pyo3::{
        prelude::*,
        types::{PyBytes, PyDict},
    };
    use serde::{Deserialize as _, Serialize as _};
    use url::Url;

    use crate::{
        serde::python_to_json_writer,
        wasm::{
            PyPollable,
            body_buffer::{BodyBuffer, Buffer},
            future::create_future,
        },
    };

    #[pyfunction]
    fn new_buffer(kind: &str) -> PyResult<ResponseBuffer> {
        let inner = Buffer::new(kind).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("invalid buffer kind: {kind}"))
        })?;
        Ok(ResponseBuffer { inner })
    }

    #[pyfunction]
    #[pyo3(signature = (method, url, params, headers, body, timeout))]
    fn fetch(
        method: &str,
        url: &str,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        body: Option<&Bound<'_, PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<PyFutureResponse> {
        enum Body<'a> {
            None,
            Bytes(Bound<'a, PyBytes>),
            Object(Bound<'a, PyAny>),
        }

        let body = body.map_or(Body::None, |body| {
            body.extract::<Bound<'_, PyBytes>>()
                .map_or(Body::Object(body.clone()), Body::Bytes)
        });

        let mut header_fields = Vec::new();
        if let Some(headers) = headers {
            for (k, v) in headers {
                let k: String = k.extract()?;
                let v: &str = v.extract()?;
                header_fields.push((k, v.as_bytes().to_vec()));
            }
        }
        if matches!(body, Body::Object(_))
            && !header_fields
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            header_fields.push(("content-type".to_string(), b"application/json".to_vec()));
        }

        let mut u = Url::parse(url)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        if let Some(params) = params {
            for (k, v) in params {
                u.query_pairs_mut().append_pair(k.extract()?, v.extract()?);
            }
        }
        let timeout_ms = timeout
            .filter(|timeout| timeout.is_finite() && *timeout > 0.0)
            .map(|timeout| std::time::Duration::from_secs_f64(timeout).as_millis())
            .map(|timeout_ms| u64::try_from(timeout_ms).unwrap_or(u64::MAX));

        let body = match &body {
            Body::None => None,
            Body::Bytes(b) => Some(b.as_bytes().to_vec()),
            Body::Object(b) => {
                let mut bytes = Vec::new();
                python_to_json_writer(b.clone(), &mut bytes)
                    .map_err(|_| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))?;
                Some(bytes)
            }
        };

        let mut request = serde_json::json!({
            "method": method,
            "url": u.to_string(),
            "headers": header_fields,
            "body": body,
        });
        if let Some(timeout_ms) = timeout_ms {
            request["timeout_ms"] = serde_json::json!(timeout_ms);
        }
        let mut payload = Vec::new();
        request
            .serialize(&mut minicbor_serde::Serializer::new(&mut payload))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        Ok(PyFutureResponse::new(crate::wasm::future::register_call(
            "__isola_http".to_string(),
            payload,
        )))
    }

    create_future!(PyFutureResponse, Result<Vec<u8>, String>, PyResponse);

    #[pyclass]
    struct PyResponse {
        status: u16,
        headers: Vec<(String, Vec<u8>)>,
        body: Vec<u8>,
        cursor: usize,
        consumed: bool,
        closed: bool,
    }

    impl TryFrom<Result<Vec<u8>, String>> for PyResponse {
        type Error = PyErr;

        fn try_from(value: Result<Vec<u8>, String>) -> Result<Self, Self::Error> {
            match value {
                Ok(response) => {
                    let mut deserializer = minicbor_serde::Deserializer::new(&response);
                    let response =
                        serde_json::Value::deserialize(&mut deserializer).map_err(|e| {
                            PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                        })?;
                    let headers = response
                        .get("headers")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|header| {
                            let pair = header.as_array()?;
                            let name = pair.first()?.as_str()?.to_string();
                            let value = pair
                                .get(1)?
                                .as_array()?
                                .iter()
                                .filter_map(serde_json::Value::as_u64)
                                .filter_map(|b| u8::try_from(b).ok())
                                .collect::<Vec<_>>();
                            Some((name, value))
                        })
                        .collect();
                    let body = response
                        .get("body")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_u64)
                        .filter_map(|b| u8::try_from(b).ok())
                        .collect();
                    Ok(Self {
                        status: response
                            .get("status")
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|s| u16::try_from(s).ok())
                            .ok_or_else(|| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                                    "missing HTTP status",
                                )
                            })?,
                        headers,
                        body,
                        cursor: 0,
                        consumed: false,
                        closed: false,
                    })
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(e)),
            }
        }
    }

    #[pymethods]
    impl PyResponse {
        const fn close(&mut self) {
            self.closed = true;
        }

        fn status(&self) -> PyResult<u16> {
            if self.closed {
                return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "response closed",
                ));
            }
            Ok(self.status)
        }

        fn headers<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyDict>> {
            let borrowed = slf.borrow();
            if borrowed.closed {
                return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "response closed",
                ));
            }
            let d = PyDict::new(slf.py());
            for (k, v) in &borrowed.headers {
                d.set_item(
                    k,
                    std::str::from_utf8(v).map_err(|_| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid header value")
                    })?,
                )?;
            }
            Ok(d)
        }

        fn read_into(
            &mut self,
            buf: &mut ResponseBuffer,
            size: i64,
        ) -> PyResult<Option<PyPollable>> {
            read_into(self, &mut buf.inner, size)
        }

        fn blocking_read<'py>(
            &mut self,
            py: Python<'py>,
            kind: &str,
            size: i64,
        ) -> PyResult<Option<Bound<'py, PyAny>>> {
            if self.consumed {
                return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "Response already read",
                ));
            }
            let mut buf = Buffer::new(kind).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!("invalid buffer kind: {kind}"))
            })?;
            while read_into(self, &mut buf, size)?.is_some() {}
            if size < 0 {
                self.consumed = true;
            }
            buf.decode_all(py)
        }
    }

    impl Drop for PyResponse {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn read_into(
        slf: &mut PyResponse,
        buf: &mut impl BodyBuffer,
        size: i64,
    ) -> PyResult<Option<PyPollable>> {
        if slf.closed {
            return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "response closed",
            ));
        }
        if slf.consumed {
            return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Response already read",
            ));
        }
        let read_size = if size < 0 {
            slf.body.len().saturating_sub(slf.cursor)
        } else {
            usize::try_from(size).expect("size is too large")
        };
        // A zero-length read makes no progress. Report completion immediately so
        // callers that loop until `None` (e.g. `blocking_read`, `_aread`) don't
        // spin forever on the always-ready pollable. The response is left
        // unconsumed so subsequent reads still work.
        if read_size == 0 && slf.cursor < slf.body.len() {
            return Ok(None);
        }
        let end = slf.cursor.saturating_add(read_size).min(slf.body.len());
        if slf.cursor < end {
            buf.write(slf.body[slf.cursor..end].to_vec());
            slf.cursor = end;
        }
        if slf.cursor >= slf.body.len() {
            buf.close();
            slf.consumed = true;
            Ok(None)
        } else {
            Ok(Some(PyPollable::default()))
        }
    }

    #[pyclass]
    struct ResponseBuffer {
        inner: Buffer,
    }

    #[pymethods]
    impl ResponseBuffer {
        fn next(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
            self.inner.decode(py).map(|o| o.map(Into::into))
        }

        fn read_all(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
            self.inner.decode_all(py).map(|o| o.map(Into::into))
        }
    }
}
