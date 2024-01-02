use crate::script::{InputValue, Script, VM};

use rustpython_vm::pymodule;
use std::cell::RefCell;

wit_bindgen::generate!({
    world: "python-vm",
    exports: {
        "vm": Global,
    },
});

pub struct Global;

impl exports::vm::Guest for Global {
    fn eval_script(script: String) -> Result<(), exports::vm::Error> {
        GLOBAL_VM.with(|vm| {
            return if let Some(vm) = vm.borrow().as_ref() {
                vm.load_script(&script).map_err(|e| match e {
                    crate::error::Error::PythonError(s) => exports::vm::Error::Python(s),
                    crate::error::Error::UnexpectedError(s) => {
                        exports::vm::Error::Unknown(s.to_owned())
                    }
                })?;
                Ok(())
            } else {
                Err(exports::vm::Error::Unknown(
                    "VM not initialized".to_string(),
                ))
            };
        })
    }

    fn call_func(func: String, args: Vec<String>) -> Result<(), exports::vm::Error> {
        GLOBAL_VM.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.iter().map(|f| InputValue::JsonStr(f)),
                        [],
                        |s| host::emit(s, false),
                    )
                    .map_err(|e| match e {
                        crate::error::Error::PythonError(s) => exports::vm::Error::Python(s),
                        crate::error::Error::UnexpectedError(s) => {
                            exports::vm::Error::Unknown(s.to_owned())
                        }
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
pub mod http {
    use rustpython_vm::{
        builtins::{PyBaseExceptionRef, PyDictRef, PyStr, PyStrRef},
        function::OptionalArg::{self, Present},
        protocol::PyIterReturn,
        py_serde::{PyObjectDeserializer, PyObjectSerializer},
        pyclass,
        types::{IterNext, SelfIter},
        Py, PyObjectRef, PyPayload, PyResult, VirtualMachine,
    };
    use serde::de::DeserializeSeed;

    use super::promptkit::python::http_client::{self, Method};

    #[pyfunction]
    fn get(
        url: PyStrRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let request = http_client::Request::new(url.as_str(), Method::Get);
        request.set_header("accept", "application/json").unwrap();
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        request
                            .set_header(k.as_str(), v.as_str())
                            .map_err(|e| into_exception(vm, e))?;
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }
        match http_client::fetch(request) {
            Ok(response) => {
                return PyObjectDeserializer::new(vm)
                    .deserialize(&mut serde_json::Deserializer::from_slice(
                        &(http_client::Response::body(response)
                            .map_err(|e| into_exception(vm, e))?),
                    ))
                    .map_err(|e| vm.new_type_error(e.to_string()));
            }
            Err(err) => Err(into_exception(vm, err)),
        }
    }

    #[pyfunction]
    fn get_sse(
        url: PyStrRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let request = http_client::Request::new(url.as_str(), Method::Get);
        request.set_header("accept", "text/event-stream").unwrap();
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        request
                            .set_header(k.as_str(), v.as_str())
                            .map_err(|e| into_exception(vm, e))?;
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }

        match http_client::fetch(request) {
            Ok(response) => Ok(SseIter {
                body: http_client::Response::body_sse(response),
            }
            .into_pyobject(vm)),
            Err(err) => Err(into_exception(vm, err)),
        }
    }

    #[pyfunction]
    fn post(
        url: PyStrRef,
        data: PyObjectRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let request = http_client::Request::new(url.as_str(), Method::Post);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "application/json").unwrap();
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        request
                            .set_header(k.as_str(), v.as_str())
                            .map_err(|e| into_exception(vm, e))?;
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }
        request.set_body(&serde_json::to_vec(&PyObjectSerializer::new(vm, &data)).unwrap());

        match http_client::fetch(request) {
            Ok(response) => PyObjectDeserializer::new(vm)
                .deserialize(&mut serde_json::Deserializer::from_slice(
                    &(http_client::Response::body(response).map_err(|e| into_exception(vm, e))?),
                ))
                .map_err(|e| vm.new_type_error(e.to_string())),
            Err(err) => Err(into_exception(vm, err)),
        }
    }

    #[pyfunction]
    fn post_sse(
        url: PyStrRef,
        data: PyObjectRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let request = http_client::Request::new(url.as_str(), Method::Post);
        request
            .set_header("content-type", "application/json")
            .unwrap();
        request.set_header("accept", "text/event-stream").unwrap();
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        request
                            .set_header(k.as_str(), v.as_str())
                            .map_err(|e| into_exception(vm, e))?;
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }
        request.set_body(&serde_json::to_vec(&PyObjectSerializer::new(vm, &data)).unwrap());

        match http_client::fetch(request) {
            Ok(response) => Ok(SseIter {
                body: http_client::Response::body_sse(response),
            }
            .into_pyobject(vm)),
            Err(err) => Err(into_exception(vm, err)),
        }
    }

    #[pyclass(no_attr, name = "SSEIterator")]
    #[derive(PyPayload, Debug)]
    struct SseIter {
        body: http_client::ResponseSseBody,
    }

    #[pyclass(with(IterNext))]
    impl SseIter {}

    impl SelfIter for SseIter {}
    impl IterNext for SseIter {
        fn next(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyIterReturn> {
            match zelf.body.read() {
                Some(Ok((_, _, data))) => {
                    if data == "[DONE]" {
                        while zelf.body.read().is_some() {}
                        Ok(PyIterReturn::StopIteration(None))
                    } else {
                        Ok(PyIterReturn::Return(
                            PyObjectDeserializer::new(vm)
                                .deserialize(&mut serde_json::Deserializer::from_str(&data))
                                .map_err(|e| vm.new_type_error(e.to_string()))?,
                        ))
                    }
                }
                Some(Err(err)) => Err(into_exception(vm, err)),
                None => Ok(PyIterReturn::StopIteration(None)),
            }
        }
    }

    fn into_exception(vm: &VirtualMachine, err: http_client::Error) -> PyBaseExceptionRef {
        match err {
            http_client::Error::Unknown(err) => vm.new_value_error(err),
        }
    }
}

thread_local! {
    static GLOBAL_VM: RefCell<Option<Script>> = RefCell::new(None);
}

#[export_name = "wizer.initialize"]
pub extern "C" fn init() {
    GLOBAL_VM.with(|vm| {
        let v = VM::new(|vm| vm.add_native_module("http", Box::new(http::make_module)));
        let s = v.script("import json, re").unwrap();
        vm.borrow_mut().replace(s);
    });
}
