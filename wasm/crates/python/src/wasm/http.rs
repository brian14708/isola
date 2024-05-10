use std::io::{BufRead, BufReader, Read};

use eventsource::event::{parse_event_line, Event};
use futures::AsyncReadExt;
use pyo3::{
    prelude::*,
    types::{PyBytes, PyDict, PyString},
};
use serde::de::DeserializeSeed;
use url::Url;
use wasi_runtime::{
    futures::{block_on, Reactor},
    streams::{AsyncStreamReader, BlockingStreamReader},
};

use crate::{
    serde::{PyObjectDeserializer, PyObjectSerializer},
    wasm::promptkit::http::client::{self, Method},
};

use super::promptkit::http::client::{HttpError, Request};

#[pymodule]
#[pyo3(name = "http")]
pub fn http_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyfn(module)]
    #[pyo3(signature = (url, params=None, headers=None, timeout=None, *, response="json", validate_status=true))]
    fn get(
        py: Python<'_>,
        url: &str,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
        validate_status: bool,
    ) -> PyResult<PyObject> {
        let (request, extract) = build_request(
            py,
            Method::Get,
            url,
            params,
            headers,
            timeout,
            response,
            None,
        )?;
        blocking_fetch(py, request, validate_status, &extract)
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, params=None, headers=None, timeout=None, *, response="json", validate_status=true))]
    fn get_async(
        py: Python<'_>,
        url: &str,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
        validate_status: bool,
    ) -> PyResult<PyObject> {
        let (request, extract) = build_request(
            py,
            Method::Get,
            url,
            params,
            headers,
            timeout,
            response,
            None,
        )?;
        Ok(AsyncResponse::new_py(py, request, extract, validate_status))
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, params=None, headers=None, timeout=None, *))]
    fn get_sse(
        py: Python<'_>,
        url: &str,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        get(py, url, params, headers, timeout, "sse", true)
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, data=None, headers=None, timeout=None, *, response="json", validate_status=true))]
    fn post(
        py: Python<'_>,
        url: &str,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
        validate_status: bool,
    ) -> PyResult<PyObject> {
        let (request, extract) = build_request(
            py,
            Method::Post,
            url,
            None,
            headers,
            timeout,
            response,
            data,
        )?;
        blocking_fetch(py, request, validate_status, &extract)
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, data=None, headers=None, timeout=None, *, response="json", validate_status=true))]
    fn post_async(
        py: Python<'_>,
        url: &str,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
        response: &str,
        validate_status: bool,
    ) -> PyResult<PyObject> {
        let (request, extract) = build_request(
            py,
            Method::Post,
            url,
            None,
            headers,
            timeout,
            response,
            data,
        )?;
        Ok(AsyncResponse::new_py(py, request, extract, validate_status))
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, data=None, headers=None, timeout=None, *))]
    fn post_sse(
        py: Python<'_>,
        url: &str,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        post(py, url, data, headers, timeout, "sse", true)
    }

    #[pyfn(module)]
    #[pyo3(signature = (requests, *, ignore_error=false))]
    fn fetch_all(
        py: Python<'_>,
        mut requests: Vec<PyRefMut<AsyncResponse>>,
        ignore_error: bool,
    ) -> PyResult<Vec<PyObject>> {
        block_on(|reactor| async move {
            let f = requests.iter_mut().map(|r| r.fetch(py, &reactor));
            if ignore_error {
                Ok(futures::future::join_all(f)
                    .await
                    .into_iter()
                    .map(|e| e.unwrap_or_else(|e| e.into_py(py)))
                    .collect())
            } else {
                futures::future::try_join_all(f).await
            }
        })
    }

    Ok(())
}

#[pyclass]
struct AsyncResponse {
    request: Option<Request>,
    extract: ResponseExtractor,
    validate_status: bool,
}

impl AsyncResponse {
    fn new_py(
        py: Python<'_>,
        request: Request,
        extract: ResponseExtractor,
        validate_status: bool,
    ) -> PyObject {
        Self {
            request: Some(request),
            extract,
            validate_status,
        }
        .into_py(py)
    }

    async fn fetch(&mut self, py: Python<'_>, reactor: &Reactor) -> PyResult<PyObject> {
        let resp = client::fetch(self.request.take().unwrap())
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        reactor.wait_for(resp.subscribe()).await;
        let resp = resp.get().unwrap().unwrap().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyTypeError, _>(match e {
                HttpError::Cancelled => "cancelled".to_string(),
                HttpError::Timeout => "timeout".to_string(),
                HttpError::StatusCode(code) => format!("status code: {code}"),
                HttpError::Unknown(s) => s,
            })
        })?;
        if self.validate_status {
            let status = resp.status();
            if !(200..300).contains(&status) {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                    "status code: {status}"
                )));
            }
        };

        let body = resp.body().unwrap();
        let mut buf = Vec::new();
        AsyncStreamReader::new(body, reactor)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        self.extract.extract_body(py, &buf)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_request(
    py: Python<'_>,
    method: client::Method,
    url: &str,
    params: Option<&Bound<'_, PyDict>>,
    headers: Option<&Bound<'_, PyDict>>,
    timeout: Option<f32>,
    response: &str,
    body: Option<PyObject>,
) -> PyResult<(client::Request, ResponseExtractor)> {
    let extract = ResponseExtractor::from_str(response)?;
    let request = client::Request::new(method);
    extract.set_accept_header(&request);

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
        request.set_timeout((timeout * 1000.0) as u64);
    }

    if let Some(body) = body {
        request
            .set_header("content-type", "application/json")
            .unwrap();
        let json = PyObjectSerializer::to_json(body.into_bound(py))
            .map_err(|_e| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))?;
        request
            .write_body(&json)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
    }

    Ok((request, extract))
}

#[derive(Clone)]
enum ResponseExtractor {
    Json,
    Text,
    Binary,
    Sse,
}

impl ResponseExtractor {
    fn from_str(format: &str) -> PyResult<Self> {
        match format {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "binary" => Ok(Self::Binary),
            "sse" => Ok(Self::Sse),
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
                    Self::Sse => "text/event-stream",
                },
            )
            .unwrap();
    }

    fn extract_body(&self, py: Python<'_>, body: &[u8]) -> PyResult<PyObject> {
        match self {
            Self::Text => Ok(PyString::new_bound(
                py,
                std::str::from_utf8(body)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?,
            )
            .into()),
            Self::Binary => Ok(PyBytes::new_bound(py, body).into()),
            Self::Json => PyObjectDeserializer::new(py)
                .deserialize(&mut serde_json::Deserializer::from_slice(body))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())),
            Self::Sse => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "unexpected response format",
            )),
        }
    }

    fn extract_blocking(
        &self,
        py: Python<'_>,
        mut body: BlockingStreamReader,
    ) -> PyResult<PyObject> {
        match self {
            Self::Text | Self::Binary | Self::Json => {
                let mut buf = Vec::new();
                body.read_to_end(&mut buf)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                self.extract_body(py, &buf)
            }
            Self::Sse => Ok(SseIter {
                response: BufReader::new(body),
            }
            .into_py(py)),
        }
    }
}

fn blocking_fetch(
    py: Python<'_>,
    request: Request,
    validate_status: bool,
    extract: &ResponseExtractor,
) -> PyResult<PyObject> {
    let resp = client::fetch(request)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
    resp.subscribe().block();
    let resp = resp.get().unwrap().unwrap().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyTypeError, _>(match e {
            HttpError::Cancelled => "cancelled".to_string(),
            HttpError::Timeout => "timeout".to_string(),
            HttpError::StatusCode(code) => format!("status code: {code}"),
            HttpError::Unknown(s) => s,
        })
    })?;
    if validate_status {
        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                "status code: {status}"
            )));
        }
    };

    let body = resp.body().unwrap();
    extract.extract_blocking(py, BlockingStreamReader::new(body))
}

#[pyclass]
struct SseIter {
    response: BufReader<BlockingStreamReader>,
}

#[pymethods]
impl SseIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> PyResult<Option<PyObject>> {
        let mut evt = Event::new();
        let mut buf = String::new();
        loop {
            buf.clear();
            let line = slf
                .response
                .read_line(&mut buf)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            if line == 0 {
                return Ok(None);
            }

            match parse_event_line(&buf, &mut evt) {
                eventsource::event::ParseResult::Dispatch => {
                    return Ok(if evt.data.trim() == "[DONE]" {
                        None
                    } else {
                        Some(
                            PyObjectDeserializer::new(slf.py())
                                .deserialize(&mut serde_json::Deserializer::from_str(&evt.data))
                                .map_err(|e| {
                                    PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                                })?,
                        )
                    })
                }
                eventsource::event::ParseResult::Next
                | eventsource::event::ParseResult::SetRetry(_) => {}
            }
        }
    }
}
