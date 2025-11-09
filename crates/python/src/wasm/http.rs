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

    use crate::serde::python_to_json_writer;
    use crate::wasm::{
        PyPollable,
        body_buffer::{BodyBuffer, Buffer},
        future::create_future,
        promptkit::script::outgoing_websocket::{
            self, ConnectRequest as WsConnectRequest, ErrorCode as WsErrorCode, FutureWebsocket,
            MessageType, ReadStream, WebsocketConnection, WebsocketMessage, WriteStream,
        },
        wasi::{
            http::{
                outgoing_handler::{
                    ErrorCode, FutureIncomingResponse, OutgoingRequest, RequestOptions, handle,
                },
                types::{Fields, IncomingBody, IncomingResponse, Method, OutgoingBody, Scheme},
            },
            io::{
                poll::Pollable,
                streams::{InputStream, StreamError},
            },
        },
    };

    #[pyfunction]
    fn new_buffer(kind: &str) -> ResponseBuffer {
        ResponseBuffer {
            inner: Buffer::new(kind),
        }
    }

    #[pyfunction]
    #[pyo3(signature = (url, headers, timeout))]
    fn ws_connect(
        url: &str,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f64>,
    ) -> PyResult<PyFutureWebsocket> {
        let mut hdrs = vec![];
        if let Some(headers) = headers {
            for (k, v) in headers {
                let k: String = k.extract()?;
                let v: String = v.extract()?;
                hdrs.push((k, v));
            }
        }
        let req = WsConnectRequest::new(url, Some(&hdrs));
        if let Some(timeout) = timeout {
            req.set_connect_timeout(Some(
                u64::try_from(std::time::Duration::from_secs_f64(timeout).as_nanos())
                    .expect("duration is too large"),
            ))
            .map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "Timeout value is too large or invalid",
                )
            })?;
        }
        let c = outgoing_websocket::connect(req);
        Ok(PyFutureWebsocket::new(c))
    }

    create_future!(PyFutureWebsocket, FutureWebsocket, PyWebsocket);

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
                .append("content-type", b"application/json")
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        }

        let request = OutgoingRequest::new(header_fields);
        request.set_method(&to_method(method)).map_err(|()| {
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
            opt.set_first_byte_timeout(Some(
                u64::try_from(std::time::Duration::from_secs_f64(timeout).as_nanos())
                    .expect("duration is too large"),
            ))
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
                    python_to_json_writer(b, BufWriter::new(&mut os)).map_err(|_e| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")
                    })?;
                }
            }

            drop(os);
            OutgoingBody::finish(ob, None).map_err(|_e| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("invalid body".to_string())
            })?;
        }

        Ok(PyFutureResponse::new(resp))
    }

    fn to_method(method: &str) -> Method {
        match method {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "DELETE" => Method::Delete,
            "HEAD" => Method::Head,
            "PATCH" => Method::Patch,
            "PUT" => Method::Put,
            m => Method::Other(m.to_string()),
        }
    }

    create_future!(PyFutureResponse, FutureIncomingResponse, PyResponse);

    #[pyclass]
    struct PyResponse {
        stream: Option<InputStream>,
        body: Option<IncomingBody>,
        response: Option<IncomingResponse>,
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

        fn headers<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyDict>> {
            let hdrs = slf
                .borrow()
                .response
                .as_ref()
                .expect("response closed")
                .headers();
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

        fn read_into(
            &mut self,
            buf: &mut ResponseBuffer,
            size: i64,
        ) -> PyResult<Option<PyPollable>> {
            read_into(self, &mut buf.inner, size).map(|p| p.map(Into::into))
        }

        fn blocking_read<'py>(
            &mut self,
            py: Python<'py>,
            kind: &str,
            size: i64,
        ) -> PyResult<Option<Bound<'py, PyAny>>> {
            let mut buf = Buffer::new(kind);
            while let Some(p) = read_into(self, &mut buf, size)? {
                p.block();
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
    ) -> PyResult<Option<Pollable>> {
        let stream = if let Some(b) = slf.stream.as_mut() {
            b
        } else {
            slf.body = Some(
                slf.response
                    .as_mut()
                    .expect("response closed")
                    .consume()
                    .map_err(|()| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>("Response already read")
                    })?,
            );
            slf.stream = Some(slf.body.as_ref().unwrap().stream().map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("Response already read")
            })?);
            slf.stream.as_mut().unwrap()
        };

        let read_size = if size < 0 {
            16384
        } else {
            u64::try_from(size).expect("size is too large")
        };
        loop {
            match InputStream::read(stream, read_size) {
                Ok(v) => {
                    if !v.is_empty() {
                        buf.write(v);
                        if size < 0 {
                            continue;
                        }
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
        fn next(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
            self.inner.decode(py).map(|o| o.map(Into::into))
        }

        fn read_all(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
            self.inner.decode_all(py).map(|o| o.map(Into::into))
        }
    }

    #[pyclass]
    struct PyWebsocket {
        read_stream: Option<ReadStream>,
        write_stream: Option<WriteStream>,
        connection: Option<WebsocketConnection>,
    }

    impl TryFrom<Result<WebsocketConnection, WsErrorCode>> for PyWebsocket {
        type Error = PyErr;

        fn try_from(result: Result<WebsocketConnection, WsErrorCode>) -> Result<Self, Self::Error> {
            match result {
                Ok(connection) => {
                    let (write_stream, read_stream) = connection.streams().unwrap();
                    Ok(Self {
                        connection: Some(connection),
                        read_stream: Some(read_stream),
                        write_stream: Some(write_stream),
                    })
                }
                Err(error) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                    format!("WebSocket connection failed: {error:?}"),
                )),
            }
        }
    }

    #[pymethods]
    impl PyWebsocket {
        fn recv<'py>(
            &mut self,
            py: Python<'py>,
        ) -> PyResult<(bool, Option<Bound<'py, PyAny>>, Option<PyPollable>)> {
            if let Some(read_stream) = &self.read_stream {
                match read_stream.read() {
                    Some(Ok(message)) => {
                        let data = message.read();
                        let content = match message.message_type() {
                            MessageType::Text => {
                                let text = std::str::from_utf8(&data)?;
                                pyo3::types::PyString::new(py, text).into_any()
                            }
                            MessageType::Binary => pyo3::types::PyBytes::new(py, &data).into_any(),
                        };
                        Ok((true, Some(content), None))
                    }
                    None => Ok((true, None, Some(PyPollable::from(read_stream.subscribe())))),
                    Some(Err(WsErrorCode::Closed((code, reason)))) => {
                        if matches!(code, 1000 | 1001 | 1005) {
                            self.read_stream.take();
                            Ok((false, None, None))
                        } else {
                            Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                                format!(
                                    "WebSocket closed with code {code:?} and reason: {reason:?}"
                                ),
                            ))
                        }
                    }
                    Some(Err(error)) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                        format!("WebSocket error: {error:?}"),
                    )),
                }
            } else {
                Ok((false, None, None))
            }
        }

        fn send(&mut self, obj: &Bound<'_, PyAny>) -> PyResult<Option<PyPollable>> {
            let message = if let Ok(s) = obj.extract::<&str>() {
                WebsocketMessage::new(MessageType::Text, s.as_bytes())
            } else if let Ok(b) = obj.extract::<&[u8]>() {
                WebsocketMessage::new(MessageType::Binary, b)
            } else {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "Unsupported type for WebSocket message, expected str or bytes",
                ));
            };

            if let Some(write_stream) = &self.write_stream {
                match write_stream.check_write(&message) {
                    Ok(true) => {
                        write_stream.write(message).map_err(|e| {
                            PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
                                "Failed to write to WebSocket: {e:?}"
                            ))
                        })?;
                        Ok(None)
                    }
                    Ok(false) => Ok(Some(PyPollable::from(write_stream.subscribe()))),
                    Err(error) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                        format!("WebSocket send failed: {error:?}"),
                    )),
                }
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                    "WebSocket connection closed",
                ))
            }
        }

        fn close(&mut self, code: u16, reason: &str) -> PyResult<Option<PyPollable>> {
            if let Some(write_stream) = &self.write_stream {
                match write_stream.close(code, reason) {
                    None => Ok(Some(PyPollable::from(write_stream.subscribe()))),
                    Some(Ok(())) => {
                        self.write_stream.take();
                        Ok(None)
                    }
                    Some(Err(error)) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                        format!("WebSocket send failed: {error:?}"),
                    )),
                }
            } else {
                Ok(None)
            }
        }

        fn shutdown(&mut self) {
            self.read_stream.take();
            self.write_stream.take();
            self.connection.take();
        }

        fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
            if let Some(connection) = &self.connection {
                let d = PyDict::new(py);
                for (k, v) in connection.headers() {
                    d.set_item(k, v)?;
                }
                Ok(d)
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                    "WebSocket connection closed",
                ))
            }
        }
    }

    impl Drop for PyWebsocket {
        fn drop(&mut self) {
            self.shutdown();
        }
    }
}
