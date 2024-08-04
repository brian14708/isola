#![allow(clippy::missing_safety_doc, clippy::module_name_repetitions)]

mod body_buffer;
mod future;
mod http;
mod logging;

use std::cell::RefCell;

use cbor4ii::core::utils::SliceReader;
use future::PyPollable;
use pyo3::{append_to_inittab, intern, prelude::*};
use serde::de::DeserializeSeed;
use wasi::{
    clocks::monotonic_clock::subscribe_duration,
    io::{poll::poll as host_poll, streams::StreamError},
};

use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde::PyObjectDeserializer,
};

use self::{exports::promptkit::vm::guest, promptkit::vm::host};

wit_bindgen::generate!({
    world: "sandbox",
    path: "../../../apis/wit",
    with: {
        "wasi:io/poll@0.2.0": wasi::io::poll,
        "wasi:io/error@0.2.0": wasi::io::error,
        "wasi:io/streams@0.2.0": wasi::io::streams,
        "wasi:clocks/monotonic-clock@0.2.0": wasi::clocks::monotonic_clock,
        "wasi:http/types@0.2.0": wasi::http::types,
        "wasi:http/outgoing-handler@0.2.0": wasi::http::outgoing_handler,

        "promptkit:vm/host": generate,
        "promptkit:vm/guest": generate,
        "promptkit:http/client": generate,
    },
});

#[pymodule]
#[pyo3(name = "_promptkit_sys")]
pub mod sys_module {
    #[allow(clippy::wildcard_imports)]
    use super::*;

    #[pymodule_export]
    use super::PyPollable;

    #[pyfunction]
    #[pyo3(signature = (duration))]
    fn sleep(duration: f64) -> PyPollable {
        let poll = subscribe_duration(
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            {
                (duration * 1_000_000_000.0) as u64
            },
        );
        poll.into()
    }

    #[pyfunction]
    #[pyo3(signature = (poll))]
    #[allow(clippy::needless_pass_by_value)]
    fn poll(poll: Vec<PyRef<'_, PyPollable>>) -> Vec<u32> {
        let p = poll.iter().map(|p| p.get_pollable()).collect::<Vec<_>>();
        host_poll(&p)
    }
}

export!(Global);

pub struct Global;

impl guest::Guest for Global {
    fn set_log_level(level: Option<host::LogLevel>) {
        logging::set_log_level(level);
    }

    fn eval_bundle(bundle_path: String, entrypoint: String) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow_mut(|vm| {
            if let Some(vm) = vm.as_mut() {
                vm.load_zip(&bundle_path, &entrypoint)
                    .map_err(Into::<guest::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn eval_script(script: String) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                vm.load_script(&script).map_err(Into::<guest::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn call_func(func: String, args: Vec<host::Argument>) -> Result<Option<Vec<u8>>, guest::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                let ret = if func == "$analyze" {
                    if let Some(host::Argument::Cbor(s)) = args.first() {
                        vm.analyze(InputValue::Cbor(s.into()))
                            .map_err(Into::<guest::Error>::into)
                    } else {
                        return Err(Error::UnexpectedError("Invalid argument").into());
                    }
                } else {
                    vm.run(
                        &func,
                        args.into_iter().map(|f| match f {
                            host::Argument::Cbor(s) => InputValue::Cbor(s.into()),
                            host::Argument::Iterator(e) => InputValue::Iter(ArgIter { iter: e }),
                        }),
                        [],
                        host::emit,
                    )
                    .map_err(Into::<guest::Error>::into)
                };
                vm.flush();
                ret
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }
}

#[pyclass]
pub struct ArgIter {
    iter: host::ArgumentIterator,
}

#[pymethods]
impl ArgIter {
    fn __iter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        match self.iter.blocking_read() {
            Ok(a) => match a {
                host::Argument::Cbor(c) => {
                    let c = SliceReader::new(&c);
                    Ok(Some(
                        PyObjectDeserializer::new(py)
                            .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                            .map_err(|_| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")
                            })?
                            .to_object(py),
                    ))
                }
                host::Argument::Iterator(_) => todo!(),
            },
            Err(StreamError::Closed) => Ok(None),
            Err(StreamError::LastOperationFailed(e)) => Err(PyErr::new::<
                pyo3::exceptions::PyTypeError,
                _,
            >(e.to_string())),
        }
    }

    fn __aiter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, PyAny>> {
        let py = slf.py();
        let module = py
            .import_bound(intern!(py, "promptkit.asyncio"))
            .expect("failed to import promptkit.asyncio");
        module
            .getattr(intern!(py, "_aiter_arg"))
            .expect("failed to get asyncio.aiter_arg")
            .call1((slf,))
    }

    fn read(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        match self.iter.read() {
            Some(Ok(a)) => match a {
                host::Argument::Cbor(c) => {
                    let c = SliceReader::new(&c);
                    Ok(Some(
                        PyObjectDeserializer::new(py)
                            .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                            .map_err(|_| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")
                            })?
                            .to_object(py),
                    ))
                }
                host::Argument::Iterator(_) => todo!(),
            },
            Some(Err(StreamError::Closed)) => Ok(None),
            Some(Err(StreamError::LastOperationFailed(e))) => Err(PyErr::new::<
                pyo3::exceptions::PyTypeError,
                _,
            >(e.to_string())),
            None => Ok(Some(PyPollable::from(self.iter.subscribe()).into_py(py))),
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
        use http::http_module;
        append_to_inittab!(http_module);
        use logging::logging_module;
        append_to_inittab!(logging_module);
        append_to_inittab!(sys_module);

        let v = Scope::new();
        let code = include_str!("prelude.py");
        v.load_script(code).unwrap();
        v.flush();
        scope.borrow_mut().replace(v);
    });
}
