#[pyo3::pymodule]
#[pyo3(name = "_isola_http")]
pub mod http_module {
    use isola_runtime::wasi_http::{HttpRequest, HttpResponse};
    use pyo3::{
        prelude::*,
        types::{PyBytes, PyDict},
    };
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
    #[pyo3(signature = (capacity = 16))]
    fn open_upload(capacity: usize) -> u32 {
        isola_runtime::pending::register_http_upload(capacity)
    }

    #[pyfunction]
    fn write_upload(handle: u32, body: &Bound<'_, PyAny>) -> PyResult<PyUploadWrite> {
        let bytes = body
            .extract::<Bound<'_, PyBytes>>()
            .map_err(|_| pyo3::exceptions::PyTypeError::new_err("upload chunk must be bytes"))?;
        let handle = isola_runtime::pending::register_http_upload_write(
            handle,
            Ok(bytes.as_bytes().to_vec()),
        )
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(PyUploadWrite { handle })
    }

    #[pyfunction]
    fn close_upload(handle: u32) -> PyResult<()> {
        isola_runtime::pending::close_http_upload(handle)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
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

        Ok(PyFutureResponse::new(crate::wasm::future::register_http(
            HttpRequest::new(method.to_string(), u, header_fields, body, timeout_ms),
        )))
    }

    #[pyfunction]
    #[pyo3(signature = (method, url, params, headers, upload_handle, timeout))]
    fn fetch_stream(
        method: &str,
        url: &str,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        upload_handle: u32,
        timeout: Option<f64>,
    ) -> PyResult<PyFutureResponse> {
        let mut header_fields = Vec::new();
        if let Some(headers) = headers {
            for (k, v) in headers {
                let k: String = k.extract()?;
                let v: &str = v.extract()?;
                header_fields.push((k, v.as_bytes().to_vec()));
            }
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
        let body = isola_runtime::pending::take_http_upload_stream(upload_handle)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        Ok(PyFutureResponse::new(crate::wasm::future::register_http(
            HttpRequest::new_stream(method.to_string(), u, header_fields, body, timeout_ms),
        )))
    }

    #[pyclass]
    struct PyUploadWrite {
        handle: u32,
    }

    #[pymethods]
    impl PyUploadWrite {
        fn wait(&self) -> PyResult<()> {
            crate::wasm::future::drive_one_http_upload_write(self.handle)
                .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        }

        fn subscribe(&self) -> Option<PyPollable> {
            let pollable = PyPollable::operation(self.handle);
            (!pollable.is_ready()).then_some(pollable)
        }

        fn get(&self) -> PyResult<()> {
            crate::wasm::future::take_http_upload_write_result(self.handle)
                .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        }

        fn release(&self) {
            crate::wasm::future::release_call(self.handle);
        }
    }

    create_future!(PyFutureResponse, http -> PyResponse);

    #[pyclass]
    struct PyResponse {
        status: u16,
        headers: Vec<(String, Vec<u8>)>,
        stream_handle: Option<u32>,
        chunk: Vec<u8>,
        cursor: usize,
        consumed: bool,
        closed: bool,
        pending_read: Option<u32>,
    }

    impl TryFrom<Result<HttpResponse, String>> for PyResponse {
        type Error = PyErr;

        fn try_from(value: Result<HttpResponse, String>) -> Result<Self, Self::Error> {
            match value {
                Ok(response) => {
                    let stream_handle = isola_runtime::pending::register_http_stream(response.body);
                    Ok(Self {
                        status: response.status,
                        headers: response.headers,
                        stream_handle: Some(stream_handle),
                        chunk: Vec::new(),
                        cursor: 0,
                        consumed: false,
                        closed: false,
                        pending_read: None,
                    })
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(e)),
            }
        }
    }

    #[pymethods]
    impl PyResponse {
        fn close(&mut self) {
            if self.closed {
                return;
            }
            self.closed = true;
            if let Some(handle) = self.pending_read.take() {
                isola_runtime::pending::release(handle);
            }
            if let Some(handle) = self.stream_handle.take() {
                let _ = isola_runtime::pending::release_http_stream(handle);
            }
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
            while let Some(pollable) = read_into(self, &mut buf, size)? {
                pollable.wait_blocking()?;
            }
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

    /// Decode a CBOR-transported byte array (serialized as a sequence of
    /// integers) back into raw bytes.
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
            usize::MAX
        } else {
            usize::try_from(size).map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyOverflowError, _>(
                    "read size is too large for this platform",
                )
            })?
        };
        // A zero-length read makes no progress. Report completion immediately
        // so callers that loop until `None` (e.g. `blocking_read`,
        // `_aread`) don't spin forever on the always-ready pollable.
        // The response is left unconsumed so subsequent reads still
        // work.
        if read_size == 0 {
            return Ok(None);
        }

        if let Some(handle) = slf.pending_read {
            if !isola_runtime::pending::is_ready(handle) {
                return Ok(Some(PyPollable::operation(handle)));
            }
            let result = match isola_runtime::pending::take(handle) {
                Ok(isola_runtime::pending::Take::Ready(
                    isola_runtime::pending::Output::HttpStream(result),
                )) => result,
                Ok(_) => Err("invalid HTTP response stream operation".to_string()),
                Err(error) => Err(error.to_string()),
            }
            .map_err(PyErr::new::<pyo3::exceptions::PyTypeError, _>)?;
            slf.pending_read = None;
            if let Some(chunk) = result {
                slf.chunk = chunk;
                slf.cursor = 0;
            } else {
                if let Some(stream_handle) = slf.stream_handle.take() {
                    let _ = isola_runtime::pending::release_http_stream(stream_handle);
                }
                slf.consumed = true;
                buf.close();
                return Ok(None);
            }
        }

        if slf.cursor < slf.chunk.len() {
            let end = slf.cursor.saturating_add(read_size).min(slf.chunk.len());
            buf.write(&slf.chunk[slf.cursor..end]);
            slf.cursor = end;
            if slf.cursor < slf.chunk.len() {
                return Ok(Some(PyPollable::default()));
            }
            slf.chunk.clear();
            slf.cursor = 0;
        }

        if let Some(stream_handle) = slf.stream_handle {
            let read_handle = isola_runtime::pending::register_http_stream_read(stream_handle);
            slf.pending_read = Some(read_handle);
            Ok(Some(PyPollable::operation(read_handle)))
        } else {
            buf.close();
            slf.consumed = true;
            Ok(None)
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
