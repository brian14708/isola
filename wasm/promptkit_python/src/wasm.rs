use std::cell::RefCell;

use pyo3::append_to_inittab;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::types::PyString;
use serde::de::DeserializeSeed;

use self::exports::vm::Argument;
use self::promptkit::python::http_client::Request;
use crate::error::Error;
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
                    .map_err(|e| exports::vm::Error::Python(e.to_string()))?;
                Ok(())
            } else {
                Err(exports::vm::Error::Unknown(
                    "VM not initialized".to_string(),
                ))
            };
        })
    }

    fn call_func(func: String, args: Vec<Argument>) -> Result<(), exports::vm::Error> {
        GLOBAL_SCOPE.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.iter().map(|f| match f {
                            Argument::Json(s) => InputValue::JsonStr(s),
                        }),
                        [],
                        |s| host::emit(s, false),
                    )
                    .map_err(|e| match e {
                        Error::PythonError { cause, traceback } => {
                            exports::vm::Error::Python(if let Some(traceback) = traceback {
                                format!("{cause}\n\n{traceback}")
                            } else {
                                cause
                            })
                        }
                        Error::UnexpectedError(e) => exports::vm::Error::Unknown(e.to_string()),
                    })?;
                host::emit(ret.as_deref().unwrap_or(""), true);
                Ok(())
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

fn set_headers(req: &Request, headers: &PyDict) -> PyResult<()> {
    for (k, v) in headers {
        let k = k.downcast::<PyString>();
        let v = v.downcast::<PyString>();

        match (k, v) {
            (Ok(k), Ok(v)) => {
                req.set_header(k.to_str()?, v.to_str()?)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            }
            _ => {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "invalid headers".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

#[pymodule]
fn http(_py: Python<'_>, module: &PyModule) -> PyResult<()> {
    #[pyfn(module)]
    fn get(py: Python<'_>, url: &PyString, headers: Option<&PyDict>) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Get);
        request.set_eager(true);
        request.set_header("accept", "application/json").unwrap();
        if let Some(headers) = headers {
            set_headers(&request, headers)?;
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
    fn get_async(py: Python<'_>, url: &PyString, headers: Option<&PyDict>) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Get);
        request.set_eager(true);
        request.set_header("accept", "application/json").unwrap();
        if let Some(headers) = headers {
            set_headers(&request, headers)?;
        }
        Ok(AsyncRequest {
            request: Some(request),
        }
        .into_py(py))
    }

    #[pyfn(module)]
    fn get_sse(py: Python<'_>, url: &PyString, headers: Option<&PyDict>) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Get);
        request.set_header("accept", "text/event-stream").unwrap();
        if let Some(headers) = headers {
            set_headers(&request, headers)?;
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
    fn post(
        py: Python<'_>,
        url: &PyString,
        body: Option<PyObject>,
        headers: Option<&PyDict>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request.set_eager(true);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "application/json").unwrap();
        if let Some(headers) = headers {
            set_headers(&request, headers)?;
        }
        if let Some(body) = body {
            request.set_body(
                &serde_json::to_vec(&PyObjectSerializer::new(py, body.as_ref(py))).unwrap(),
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
    fn post_async(
        py: Python<'_>,
        url: &PyString,
        body: Option<PyObject>,
        headers: Option<&PyDict>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request.set_eager(true);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "application/json").unwrap();
        if let Some(headers) = headers {
            set_headers(&request, headers)?;
        }
        if let Some(body) = body {
            request.set_body(
                &serde_json::to_vec(&PyObjectSerializer::new(py, body.as_ref(py))).unwrap(),
            );
        }
        Ok(AsyncRequest {
            request: Some(request),
        }
        .into_py(py))
    }

    #[pyfn(module)]
    fn post_sse(
        py: Python<'_>,
        url: &PyString,
        body: Option<PyObject>,
        headers: Option<&PyDict>,
    ) -> PyResult<PyObject> {
        let request = http_client::Request::new(url.to_str()?, Method::Post);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "text/event-stream").unwrap();
        if let Some(headers) = headers {
            set_headers(&request, headers)?;
        }
        if let Some(body) = body {
            request.set_body(
                &serde_json::to_vec(&PyObjectSerializer::new(py, body.as_ref(py))).unwrap(),
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
        v.load_script("").unwrap();
        scope.borrow_mut().replace(v);
    });
}
