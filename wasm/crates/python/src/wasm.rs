#![allow(clippy::missing_safety_doc)]

use std::{borrow::Cow, cell::RefCell};

use cbor4ii::core::utils::SliceReader;
use pyo3::{
    append_to_inittab,
    prelude::*,
    types::{PyDict, PyString, PyTuple},
};
use serde::de::DeserializeSeed;
use url::Url;

use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde::{PyLogDict, PyObjectDeserializer, PyObjectSerializer},
    wasm::{
        exports::promptkit::script::guest_api,
        promptkit::script::{
            host_api,
            http_client::{self, Method, Request},
        },
    },
};

wit_bindgen::generate!({
    world: "sandbox",
});

export!(Global);

pub struct Global;

impl guest_api::Guest for Global {
    fn set_log_level(level: Option<host_api::LogLevel>) {
        GLOBAL_LOGGING.with_borrow_mut(|l| match level {
            Some(level) => *l = loglevel_to_i32(level),
            None => *l = 0,
        });
    }

    fn eval_bundle(bundle_path: String, entrypoint: String) -> Result<(), guest_api::Error> {
        GLOBAL_SCOPE.with_borrow_mut(|vm| {
            if let Some(vm) = vm.as_mut() {
                vm.load_zip(&bundle_path, &entrypoint)
                    .map_err(Into::<guest_api::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn eval_script(script: String) -> Result<(), guest_api::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                vm.load_script(&script)
                    .map_err(Into::<guest_api::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn call_func(
        func: String,
        args: Vec<host_api::Argument>,
    ) -> Result<Option<Vec<u8>>, guest_api::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.into_iter().map(|f| match f {
                            host_api::Argument::Cbor(s) => InputValue::Cbor(s.into()),
                            host_api::Argument::Iterator(e) => {
                                InputValue::Iter(ArgIter { iter: e })
                            }
                        }),
                        [],
                        host_api::emit,
                    )
                    .map_err(Into::<guest_api::Error>::into);
                vm.flush();
                ret
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }
}

#[pymodule]
fn logging(_py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn debug(
        _py: Python<'_>,
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host_api::LogLevel::Debug) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host_api::emit_log(host_api::LogLevel::Debug, &m);
            }
            Ok(())
        })
    }

    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn info(
        _py: Python<'_>,
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host_api::LogLevel::Info) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host_api::emit_log(host_api::LogLevel::Info, &m);
            }
            Ok(())
        })
    }

    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn warning(
        _py: Python<'_>,
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host_api::LogLevel::Warn) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host_api::emit_log(host_api::LogLevel::Warn, &m);
            }
            Ok(())
        })
    }

    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn error(
        _py: Python<'_>,
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host_api::LogLevel::Error) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host_api::emit_log(host_api::LogLevel::Error, &m);
            }
            Ok(())
        })
    }

    Ok(())
}

#[pymodule]
#[pyo3(name = "promptkit")]
fn promptkit_module(py: Python<'_>, module: Bound<'_, PyModule>) -> PyResult<()> {
    let http_module = PyModule::new_bound(py, "http")?;
    http(py, &http_module)?;
    let logging_module = PyModule::new_bound(py, "logging")?;
    logging(py, &logging_module)?;

    module.add_submodule(&http_module)?;
    module.add_submodule(&logging_module)?;
    Ok(())
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

#[pymodule]
fn http(_py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyfn(module)]
    #[pyo3(signature = (url, /, params=None, headers=None, timeout=None))]
    fn get(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(&url_with_params(url, params)?, Method::Get);
        request.set_eager(true);
        request.set_header("accept", "application/json").unwrap();
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        match http_client::fetch(request) {
            Ok(response) => PyObjectDeserializer::new(py)
                .deserialize(&mut serde_json::Deserializer::from_slice(
                    &(http_client::Response::body(response).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                    })?),
                ))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, /, params=None, headers=None, timeout=None))]
    fn get_async(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        params: Option<&Bound<'_, PyDict>>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(&url_with_params(url, params)?, Method::Get);
        request.set_eager(true);
        request.set_header("accept", "application/json").unwrap();
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        Ok(AsyncRequest {
            request: Some(request),
        }
        .into_py(py))
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, /, params=None, headers=None, timeout=None))]
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
    #[pyo3(signature = (url, /, data=None, headers=None, timeout=None))]
    fn post(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request.set_eager(true);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "application/json").unwrap();
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        if let Some(data) = data {
            request.set_body(&PyObjectSerializer::to_json(data.into_bound(py)).unwrap());
        }
        match http_client::fetch(request) {
            Ok(response) => PyObjectDeserializer::new(py)
                .deserialize(&mut serde_json::Deserializer::from_slice(
                    &(http_client::Response::body(response).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                    })?),
                ))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, /, data=None, headers=None, timeout=None))]
    fn post_async(
        py: Python<'_>,
        url: &Bound<'_, PyString>,
        data: Option<PyObject>,
        headers: Option<&Bound<'_, PyDict>>,
        timeout: Option<f32>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request.set_eager(true);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "application/json").unwrap();
        set_headers(&request, headers)?;
        set_timeout(&request, timeout);
        if let Some(data) = data {
            request.set_body(&PyObjectSerializer::to_json(data.into_bound(py)).unwrap());
        }
        Ok(AsyncRequest {
            request: Some(request),
        }
        .into_py(py))
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, /, data=None, headers=None, timeout=None))]
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
    fn fetch_all(
        py: Python<'_>,
        mut requests: Vec<PyRefMut<AsyncRequest>>,
    ) -> PyResult<Vec<PyObject>> {
        let mut results = vec![];

        for e in http_client::fetch_all(
            requests
                .iter_mut()
                .map(|r| r.request.take().unwrap())
                .collect::<Vec<_>>(),
        ) {
            match e {
                Ok(response) => {
                    results.push(
                        PyObjectDeserializer::new(py)
                            .deserialize(&mut serde_json::Deserializer::from_slice(
                                &(http_client::Response::body(response).map_err(|e| {
                                    PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                                })?),
                            ))
                            .map_err(|e| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                            })?,
                    );
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
}

#[pyclass]
pub struct ArgIter {
    iter: host_api::ArgumentIterator,
}

#[pymethods]
impl ArgIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[allow(clippy::needless_pass_by_value)]
    fn __next__(slf: PyRefMut<'_, Self>) -> PyResult<Option<PyObject>> {
        match slf.iter.read() {
            Some(a) => match a {
                host_api::Argument::Cbor(c) => {
                    let c = SliceReader::new(&c);
                    Ok(Some(
                        PyObjectDeserializer::new(slf.py())
                            .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                            .map_err(|_| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")
                            })?,
                    ))
                }
                host_api::Argument::Iterator(_) => todo!(),
            },
            None => Ok(None),
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

thread_local! {
    static GLOBAL_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
    static GLOBAL_LOGGING: RefCell<i32> = const { RefCell::new(0) };
}

#[export_name = "wizer.initialize"]
pub extern "C" fn _initialize() {
    extern "C" {
        fn __wasm_call_ctors();
    }
    unsafe { __wasm_call_ctors() };

    GLOBAL_SCOPE.with(|scope| {
        append_to_inittab!(promptkit_module);
        let v = Scope::new();
        let code = include_str!("prelude.py");
        v.load_script(code).unwrap();
        v.flush();
        scope.borrow_mut().replace(v);
    });
}

pub const fn loglevel_to_i32(level: host_api::LogLevel) -> i32 {
    match level {
        host_api::LogLevel::Debug => -4,
        host_api::LogLevel::Info => -3,
        host_api::LogLevel::Warn => -2,
        host_api::LogLevel::Error => -1,
    }
}
