use std::borrow::Cow;

use pyo3::{
    prelude::*,
    types::{PyBytes, PyDict, PyString},
};
use serde::de::DeserializeSeed;
use url::Url;

use crate::{
    serde::{PyObjectDeserializer, PyObjectSerializer},
    wasm::promptkit::script::http_client::{self, Method, Request},
};

#[pymodule]
#[pyo3(name = "http")]
pub fn http_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyfn(module)]
    #[pyo3(signature = (url, params=None, headers=None, timeout=None, *, response="json"))]
    fn get(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(&url_with_params(url, params)?, Method::Get);
        request.set_eager(true);
        let format = ResponseFormat::from_str(response)?;
        format.set_accept_header(&request);
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        match http_client::fetch(request) {
            Ok(response) => format.response(py, response),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, params=None, headers=None, timeout=None, *, response="json"))]
    fn get_async(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(&url_with_params(url, params)?, Method::Get);
        request.set_eager(true);
        let format = ResponseFormat::from_str(response)?;
        format.set_accept_header(&request);
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        Ok(AsyncRequest {
            request: Some(request),
            format,
        }
        .into_py(py))
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, params=None, headers=None, timeout=None, *))]
    fn get_sse(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(&url_with_params(url, params)?, Method::Get);
        request.set_header("accept", "text/event-stream").unwrap();
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        match http_client::fetch(request) {
            Ok(response) => Ok(SseIter {
                body: http_client::Response::body_sse(response),
            }
            .into_py(py)),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, data=None, headers=None, timeout=None, *, response="json"))]
    fn post(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request.set_eager(true);
        let format = ResponseFormat::from_str(response)?;
        format.set_accept_header(&request);
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        if let Some(data) = data {
            request.set_body(&PyObjectSerializer::to_json(data.into_bound(py)).unwrap());
        }
        match http_client::fetch(request) {
            Ok(response) => format.response(py, response),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, data=None, headers=None, timeout=None, *, response="json"))]
    fn post_async(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request.set_eager(true);
        let format = ResponseFormat::from_str(response)?;
        format.set_accept_header(&request);
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        if let Some(data) = data {
            request.set_body(&PyObjectSerializer::to_json(data.into_bound(py)).unwrap());
        }
        Ok(AsyncRequest {
            request: Some(request),
            format,
        }
        .into_py(py))
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, data=None, headers=None, timeout=None, *))]
    fn post_sse(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "text/event-stream").unwrap();
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        if let Some(data) = data {
            request.set_body(&PyObjectSerializer::to_json(data.into_bound(py)).unwrap());
        }
        match http_client::fetch(request) {
            Ok(response) => Ok(SseIter {
                body: http_client::Response::body_sse(response),
            }
            .into_py(py)),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (requests, *, allow_errors=false))]
    fn fetch_all(
        py: Python<'_>,
        mut requests: Vec<PyRefMut<AsyncRequest>>,
        allow_errors: bool,
    ) -> PyResult<Vec<PyObject>> {
        let mut results = vec![];

        for (e, request) in http_client::fetch_all(
            requests
                .iter_mut()
                .map(|r| r.request.take().unwrap())
                .collect::<Vec<_>>(),
        )
        .into_iter()
        .zip(requests.iter())
        {
            match e {
                Ok(response) => {
                    results.push(match request.format.response(py, response) {
                        Ok(r) => r,
                        Err(e) => {
                            if allow_errors {
                                e.into_py(py)
                            } else {
                                return Err(e);
                            }
                        }
                    });
                }
                Err(e) => {
                    return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        e.to_string(),
                    ));
                }
            }
        }

        Ok(results)
    }

    Ok(())
}

#[pyclass]
struct AsyncRequest {
    request: Option<Request>,
    format: ResponseFormat,
}

fn set_headers(req: &Request, headers: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
    if let Some(headers) = headers {
        for (k, v) in headers {
            match (k.extract::<&str>(), v.extract::<&str>()) {
                (Ok(k), Ok(v)) => {
                    req.set_header(k, v).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                    })?;
                }
                _ => {
                    return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        "invalid headers".to_owned(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn url_with_params<'a>(
    url: &'a Bound<'a, PyString>,
    params: Option<&'a Bound<'a, PyDict>>,
) -> PyResult<Cow<'a, str>> {
    if let Some(params) = params {
        let mut u = Url::parse(url.to_str()?)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        for (k, v) in params {
            u.query_pairs_mut().append_pair(k.extract()?, v.extract()?);
        }
        Ok(u.to_string().into())
    } else {
        Ok(url.to_str()?.into())
    }
}

fn set_timeout(request: &Request, timeout: Option<f32>) {
    if let Some(timeout) = timeout {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        request.set_timeout((timeout * 1000.0) as u64);
    }
}

enum ResponseFormat {
    Json,
    Text,
    Binary,
}

impl ResponseFormat {
    fn from_str(format: &str) -> PyResult<Self> {
        match format {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "binary" => Ok(Self::Binary),
            _ => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "invalid response format".to_string(),
            )),
        }
    }

    fn set_accept_header(&self, request: &Request) {
        request
            .set_header(
                "accept",
                match self {
                    Self::Text => "text/*",
                    Self::Json => "application/json",
                    Self::Binary => "application/octet-stream",
                },
            )
            .unwrap();
    }

    fn response(&self, py: Python<'_>, response: http_client::Response) -> PyResult<PyObject> {
        let body = http_client::Response::body(response)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        match self {
            Self::Text => Ok(PyString::new_bound(
                py,
                std::str::from_utf8(&body)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?,
            )
            .into()),
            Self::Binary => Ok(PyBytes::new_bound(py, &body).into()),
            Self::Json => PyObjectDeserializer::new(py)
                .deserialize(&mut serde_json::Deserializer::from_slice(&body))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())),
        }
    }
}

#[pyclass]
struct SseIter {
    body: http_client::ResponseSseBody,
}

#[pymethods]
impl SseIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[allow(clippy::needless_pass_by_value)]
    fn __next__(slf: PyRefMut<'_, Self>) -> PyResult<Option<PyObject>> {
        match slf.body.read() {
            Some(Ok(event)) => {
                if event.data == "[DONE]" {
                    while slf.body.read().is_some() {}
                    Ok(None)
                } else {
                    Ok(Some(
                        PyObjectDeserializer::new(slf.py())
                            .deserialize(&mut serde_json::Deserializer::from_str(&event.data))
                            .map_err(|e| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                            })?,
                    ))
                }
            }
            Some(Err(err)) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                err.to_string(),
            )),
            None => Ok(None),
        }
    }
}
