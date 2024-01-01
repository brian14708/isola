use crate::script::{InputValue, Script, VM};

use rustpython_vm::pymodule;
use std::cell::RefCell;

wit_bindgen::generate!({
    world: "python-vm",
    exports: {
        "python-vm": Global,
    },
});

pub struct Global;

impl exports::python_vm::Guest for Global {
    fn eval_script(script: String) -> Result<(), String> {
        GLOBAL_VM.with(|vm| {
            return if let Some(vm) = vm.borrow().as_ref() {
                vm.load_script(&script).map_err(|e| e.to_string())?;
                Ok(())
            } else {
                Err("VM not initialized".to_string())
            };
        })
    }

    fn call_func(func: String, args: Vec<String>) -> Result<(), String> {
        GLOBAL_VM.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.iter().map(|f| InputValue::JsonStr(f)),
                        [],
                        |s| host::emit(s, false),
                    )
                    .map_err(|e| e.to_string())?;
                host::emit(ret.as_deref().unwrap_or(""), true);
                Ok(())
            } else {
                Err("VM not initialized".to_string())
            }
        })
    }
}

#[pymodule]
pub mod http {
    use rustpython_vm::{
        builtins::{PyDictRef, PyStr, PyStrRef},
        function::OptionalArg::{self, Present},
        protocol::PyIterReturn,
        py_serde::{PyObjectDeserializer, PyObjectSerializer},
        pyclass,
        types::{IterNext, SelfIter},
        Py, PyObjectRef, PyPayload, PyResult, VirtualMachine,
    };
    use serde::de::DeserializeSeed;

    use super::http_client::{self, Method};

    #[pyfunction]
    fn get(
        url: PyStrRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let mut h = vec![("accept".to_owned(), "application/json".to_owned())];
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        h.push((k.as_str().to_owned(), v.as_str().to_owned()));
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }

        match http_client::fetch(&(url.as_str().to_owned(), Method::Get, h, None), 0) {
            Ok((_status, _headers, body)) => {
                return PyObjectDeserializer::new(vm)
                    .deserialize(&mut serde_json::Deserializer::from_slice(&body))
                    .map_err(|e| vm.new_type_error(e.to_string()));
            }
            Err(err) => Err(vm.new_type_error(err)),
        }
    }

    #[pyfunction]
    fn get_sse(
        url: PyStrRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let mut h = vec![("accept".to_owned(), "text/event-stream".to_owned())];
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        h.push((k.as_str().to_owned(), v.as_str().to_owned()));
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }

        match http_client::fetch_sse(&(url.as_str().to_owned(), Method::Get, h, None), 0) {
            Ok((_, _, body)) => Ok(SseIter { body }.into_pyobject(vm)),
            Err(err) => Err(vm.new_type_error(err)),
        }
    }

    #[pyfunction]
    fn post(
        url: PyStrRef,
        data: PyObjectRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let mut h = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("accept".to_owned(), "application/json".to_owned()),
        ];
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        h.push((k.as_str().to_owned(), v.as_str().to_owned()));
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }

        match http_client::fetch(
            &(
                url.as_str().to_owned(),
                Method::Post,
                h,
                Some(serde_json::to_vec(&PyObjectSerializer::new(vm, &data)).unwrap()),
            ),
            0,
        ) {
            Ok((_status, _headers, body)) => PyObjectDeserializer::new(vm)
                .deserialize(&mut serde_json::Deserializer::from_slice(&body))
                .map_err(|e| vm.new_type_error(e.to_string())),
            Err(err) => Err(vm.new_type_error(err)),
        }
    }

    #[pyfunction]
    fn post_sse(
        url: PyStrRef,
        data: PyObjectRef,
        headers: OptionalArg<PyDictRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let mut h = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("accept".to_owned(), "text/event-stream".to_owned()),
        ];
        if let Present(headers) = headers {
            for (k, v) in headers.into_iter() {
                let k = k.downcast_ref::<PyStr>();
                let v = v.downcast_ref::<PyStr>();

                match (k, v) {
                    (Some(k), Some(v)) => {
                        h.push((k.as_str().to_owned(), v.as_str().to_owned()));
                    }
                    _ => {
                        return Err(vm.new_type_error("invalid headers".to_owned()));
                    }
                }
            }
        }

        match http_client::fetch_sse(
            &(
                url.as_str().to_owned(),
                Method::Post,
                h,
                Some(serde_json::to_vec(&PyObjectSerializer::new(vm, &data)).unwrap()),
            ),
            0,
        ) {
            Ok((_, _, body)) => Ok(SseIter { body }.into_pyobject(vm)),
            Err(err) => Err(vm.new_type_error(err)),
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
                Some(Err(err)) => Err(vm.new_type_error(err)),
                None => Ok(PyIterReturn::StopIteration(None)),
            }
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
