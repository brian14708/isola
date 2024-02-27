use std::borrow::Cow;
use std::cell::RefCell;

use cbor4ii::core::utils::SliceReader;
use pyo3::append_to_inittab;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::types::PyString;
use serde::de::DeserializeSeed;
use url::Url;

use self::exports::vm::Argument;
use self::promptkit::python::http_client::Request;
use self::promptkit::python::types;
use crate::script::{InputValue, Scope};
use crate::serde::{PyObjectDeserializer, PyObjectSerializer};
use crate::wasm::promptkit::python::http_client::{self, Method};

wit_bindgen::generate!({
    world: "python-vm",
    exports: {
        "vm": Global,
    },
});

pub struct Global;

impl exports::vm::Guest for Global {
    fn eval_script(script: String) -> Result<(), exports::vm::Error> {
        GLOBAL_SCOPE.with(|vm| {
            return if let Some(vm) = vm.borrow().as_ref() {
                vm.load_script(&script)
                    .map_err(Into::<exports::vm::Error>::into)
            } else {
                Err(exports::vm::Error::Unknown(
                    "VM not initialized".to_string(),
                ))
            };
        })
    }

    fn call_func(func: String, args: Vec<Argument>) -> Result<Option<Vec<u8>>, exports::vm::Error> {
        GLOBAL_SCOPE.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.into_iter().map(|f| match f {
                            Argument::Cbor(s) => InputValue::Cbor(s.into()),
                            Argument::Iterator(e) => InputValue::Iter(ArgIter { iter: e }),
                        }),
                        [],
                        host::emit,
                    )
                    .map_err(Into::<exports::vm::Error>::into)?;
                vm.flush();
                Ok(ret)
            } else {
                Err(exports::vm::Error::Unknown(
                    "VM not initialized".to_string(),
                ))
            }
        })
    }
}

#[pymodule]
#[pyo3(name = "promptkit")]
fn promptkit_module(py: Python<'_>, module: &PyModule) -> PyResult<()> {
    let http_module = PyModule::new(py, "http")?;
    http(py, http_module)?;

    module.add_submodule(http_module)?;
    Ok(())
}

fn set_headers(req: &Request, headers: Option<&PyDict>) -> PyResult<()> {
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

fn url_with_params<'a>(url: &'a PyString, params: Option<&PyDict>) -> PyResult<Cow<'a, str>> {
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
fn http(_py: Python<'_>, module: &PyModule) -> PyResult<()> {
    #[pyfn(module)]
    #[pyo3(signature = (url, /, params=None, headers=None, timeout=None))]
    fn get(
        py: Python<'_>,
        url: &PyString,
        params: Option<&PyDict>,
        headers: Option<&PyDict>,
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
        url: &PyString,
        params: Option<&PyDict>,
        headers: Option<&PyDict>,
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
        url: &PyString,
        params: Option<&PyDict>,
        headers: Option<&PyDict>,
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
        url: &PyString,
        data: Option<PyObject>,
        headers: Option<&PyDict>,
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
            request.set_body(
                &serde_json::to_vec(&PyObjectSerializer::new(py, data.as_ref(py))).unwrap(),
            );
        }
        match http_client::fetch(request) {
            Ok(response) => {
                return PyObjectDeserializer::new(py)
                    .deserialize(&mut serde_json::Deserializer::from_slice(
                        &(http_client::Response::body(response).map_err(|e| {
                            PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())
                        })?),
                    ))
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()));
            }
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                e.to_string(),
            )),
        }
    }

    #[pyfn(module)]
    #[pyo3(signature = (url, /, data=None, headers=None, timeout=None))]
    fn post_async(
        py: Python<'_>,
        url: &PyString,
        data: Option<PyObject>,
        headers: Option<&PyDict>,
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
            request.set_body(
                &serde_json::to_vec(&PyObjectSerializer::new(py, data.as_ref(py))).unwrap(),
            );
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
        url: &PyString,
        data: Option<PyObject>,
        headers: Option<&PyDict>,
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
            request.set_body(
                &serde_json::to_vec(&PyObjectSerializer::new(py, data.as_ref(py))).unwrap(),
            );
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
    iter: types::ArgumentIterator,
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
                types::Argument::Cbor(c) => {
                    let c = SliceReader::new(&c);
                    Ok(Some(
                        PyObjectDeserializer::new(slf.py())
                            .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                            .map_err(|_| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")
                            })?,
                    ))
                }
                types::Argument::Iterator(_) => todo!(),
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
            Some(Ok((_, _, data))) => {
                if data == "[DONE]" {
                    while slf.body.read().is_some() {}
                    Ok(None)
                } else {
                    Ok(Some(
                        PyObjectDeserializer::new(slf.py())
                            .deserialize(&mut serde_json::Deserializer::from_str(&data))
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
