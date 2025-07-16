#[pyo3::pymodule]
#[pyo3(name = "_promptkit_rpc")]
pub mod rpc_module {
    use pyo3::{
        PyAny,
        prelude::*,
        types::{PyBytes, PyDict, PyString},
    };

    use crate::wasm::{
        future::{PyPollable, create_future},
        promptkit::script::outgoing_rpc::{
            self, ConnectRequest, Connection, ErrorCode, FutureConnection, Payload, RequestStream,
            ResponseStream, StreamError,
        },
    };

    #[pyfunction]
    #[pyo3(signature = (url, metadata, timeout))]
    fn connect(
        url: &str,
        metadata: Option<&Bound<'_, PyDict>>,
        timeout: Option<f64>,
    ) -> PyResult<PyFutureConnection> {
        let mut md = vec![];
        if let Some(metadata) = metadata {
            for (k, v) in metadata {
                let k: String = k.extract()?;
                let v: &str = v.extract()?;
                let v = v.as_bytes().to_vec();
                md.push((k, v));
            }
        }
        let req = ConnectRequest::new(url, Some(&md));
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
        let c = outgoing_rpc::connect(req);
        Ok(PyFutureConnection::new(c))
    }

    create_future!(PyFutureConnection, FutureConnection, PyConnection);

    #[pyclass]
    struct PyConnection {
        request: Option<RequestStream>,
        response: Option<ResponseStream>,
        connection: Option<Connection>,
    }

    impl TryFrom<Result<Connection, ErrorCode>> for PyConnection {
        type Error = PyErr;

        fn try_from(result: Result<Connection, ErrorCode>) -> Result<Self, Self::Error> {
            match result {
                Ok(connection) => {
                    let (request, response) = connection.streams().unwrap();
                    Ok(Self {
                        connection: Some(connection),
                        request: Some(request),
                        response: Some(response),
                    })
                }
                Err(error) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                    format!("RPC connection failed: {error}"),
                )),
            }
        }
    }

    #[pymethods]
    impl PyConnection {
        fn recv<'py>(
            &mut self,
            py: Python<'py>,
        ) -> PyResult<(bool, Option<Bound<'py, PyAny>>, Option<PyPollable>)> {
            if let Some(response) = &self.response {
                match response.read() {
                    Some(Ok(data)) => {
                        if data.content_type().is_some_and(|t| t.starts_with("text/")) {
                            let data = PyString::new(py, std::str::from_utf8(&data.data())?);
                            Ok((true, Some(data.into_any()), None))
                        } else {
                            let data = PyBytes::new(py, &data.data());
                            Ok((true, Some(data.into_any()), None))
                        }
                    }
                    None => Ok((true, None, Some(PyPollable::from(response.subscribe())))),
                    Some(Err(StreamError::Closed)) => {
                        self.response.take();
                        Ok((false, None, None))
                    }
                    Some(Err(error)) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                        format!("RPC stream error: {error}"),
                    )),
                }
            } else {
                Ok((false, None, None))
            }
        }

        fn send(&mut self, obj: &Bound<'_, PyAny>) -> PyResult<Option<PyPollable>> {
            let payload = if let Ok(s) = obj.extract::<&str>() {
                let p = Payload::new(s.as_bytes());
                p.set_content_type("text/plain");
                p
            } else if let Ok(b) = obj.extract::<&[u8]>() {
                Payload::new(b)
            } else {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "Unsupported type for RPC payload, expected str or bytes",
                ));
            };

            if let Some(request) = &self.request {
                match request.check_write(&payload) {
                    Ok(true) => {
                        request.write(payload).map_err(|e| {
                            PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
                                "Failed to write to RPC stream: {e}"
                            ))
                        })?;
                        Ok(None)
                    }
                    Ok(false) => Ok(Some(PyPollable::from(request.subscribe()))),
                    Err(StreamError::Closed) => {
                        self.request.take();
                        Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                            "RPC connection closed",
                        ))
                    }
                    Err(error) => Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                        format!("RPC send failed: {error}"),
                    )),
                }
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyConnectionError, _>(
                    "RPC connection closed",
                ))
            }
        }

        fn close(&mut self) -> PyResult<()> {
            if let Some(r) = self.request.take() {
                return RequestStream::finish(r).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
                        "Failed to close RPC stream: {e}"
                    ))
                });
            }
            Ok(())
        }

        fn shutdown(&mut self) {
            self.request.take();
            self.response.take();
            self.connection.take();
        }
    }

    impl Drop for PyConnection {
        fn drop(&mut self) {
            self.shutdown();
        }
    }
}
