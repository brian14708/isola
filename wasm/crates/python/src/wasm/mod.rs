mod body_buffer;
mod future;
mod http;
mod logging;
mod rpc;
mod serde;

use std::cell::RefCell;

use self::wasi::io::streams::StreamError;
use self::wasi::logging::logging::Level;
use future::PyPollable;
use pyo3::{append_to_inittab, prelude::*, sync::GILOnceCell};

use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde::cbor_to_python,
};

use self::{exports::promptkit::script::guest, promptkit::script::host};

wit_bindgen::generate!({
    world: "sandbox",
    path: "../../../apis/wit",
    generate_all,
});

#[pymodule]
#[pyo3(name = "_promptkit_sys")]
pub mod sys_module {
    use std::time::Duration;

    use pyo3::{
        Bound, PyResult, intern, pyfunction,
        types::{PyAnyMethods, PyList, PyListMethods, PySet, PyTuple, PyTupleMethods},
    };

    use super::wasi::{
        clocks::monotonic_clock::{now, subscribe_duration},
        io::poll::poll as host_poll,
    };

    #[pymodule_export]
    use super::PyPollable;

    #[pyfunction]
    #[pyo3(signature = (duration))]
    fn sleep(duration: f64) -> PyPollable {
        let poll = subscribe_duration(
            u64::try_from(Duration::from_secs_f64(duration).as_nanos())
                .expect("duration is too large"),
        );
        poll.into()
    }

    #[pyfunction]
    fn monotonic() -> f64 {
        Duration::from_nanos(now()).as_secs_f64()
    }

    #[pyfunction]
    #[pyo3(signature = (poll))]
    fn poll<'py>(poll: &Bound<'py, PyList>) -> PyResult<Bound<'py, PySet>> {
        let py = poll.py();
        let mut refs = vec![];
        let mut result = vec![];
        for (i, p) in poll.iter().enumerate() {
            let p = p.downcast_exact::<PyTuple>()?;
            let p = p.get_item(0)?;
            let p = p.call_method0(intern!(py, "subscribe"))?;
            if p.is_none() {
                result.push(i);
            } else {
                let p = p.downcast_exact::<PyPollable>()?;
                refs.push((i, p.borrow_mut()));
            }
        }

        let handles = refs
            .iter()
            .map(|(_, p)| p.get_pollable())
            .collect::<Vec<_>>();
        let result = PySet::new(
            py,
            host_poll(&handles)
                .into_iter()
                .map(|idx| {
                    let (i, _) = refs[idx as usize];
                    i
                })
                .chain(result),
        )
        .unwrap();
        for (_, p) in &mut refs {
            p.release();
        }
        Ok(result)
    }
}

export!(Global);

pub struct Global;

impl guest::Guest for Global {
    fn initialize(preinit: bool) {
        GLOBAL_SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            if scope.is_none() {
                use http::http_module;
                use logging::logging_module;
                use rpc::rpc_module;
                use serde::serde_module;

                append_to_inittab!(http_module);
                append_to_inittab!(logging_module);
                append_to_inittab!(sys_module);
                append_to_inittab!(rpc_module);
                append_to_inittab!(serde_module);

                let v = Scope::new();
                let code = include_str!("prelude.py");
                v.load_script(code).unwrap();
                v.flush();
                scope.replace(v);
            }
        });

        // https://github.com/bytecodealliance/componentize-py/blob/72348e0ebd74ef1027c52528409a289765ed5c4c/runtime/src/lib.rs#L377
        if preinit {
            #[link(wasm_import_module = "wasi_snapshot_preview1")]
            unsafe extern "C" {
                #[cfg_attr(target_arch = "wasm32", link_name = "reset_adapter_state")]
                fn reset_adapter_state();
            }

            // This tells wasi-libc to reset its preopen state, forcing re-initialization at runtime.
            #[link(wasm_import_module = "env")]
            unsafe extern "C" {
                #[cfg_attr(target_arch = "wasm32", link_name = "__wasilibc_reset_preopens")]
                fn wasilibc_reset_preopens();
            }

            unsafe {
                reset_adapter_state();
                wasilibc_reset_preopens();
            }
        }
    }

    fn set_log_level(level: Option<Level>) {
        logging::set_log_level(level);
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

    fn eval_file(path: String) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                let script = std::fs::read_to_string(std::path::Path::new(&path))
                    .map_err(|_e| Error::UnexpectedError("fail to read script"))?;
                vm.load_script(&script).map_err(Into::<guest::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn call_func(
        func: String,
        args: Vec<guest::Argument>,
    ) -> Result<Option<Vec<u8>>, guest::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                let ret = if func == "$analyze" {
                    if let Some(guest::Argument {
                        name: None,
                        value: host::Value::Cbor(s),
                    }) = args.first()
                    {
                        vm.analyze(InputValue::Cbor(s.into()))
                            .map_err(Into::<guest::Error>::into)
                    } else {
                        return Err(Error::UnexpectedError("Invalid Value").into());
                    }
                } else {
                    let mut positional = vec![];
                    let mut named = vec![];
                    for arg in args {
                        let guest::Argument { name, value } = arg;
                        let value = match value {
                            host::Value::Cbor(s) => InputValue::Cbor(s.into()),
                            host::Value::CborIterator(e) => InputValue::Iter(ArgIter { iter: e }),
                        };
                        if let Some(name) = name {
                            named.push((name.into(), value));
                        } else {
                            positional.push(value);
                        }
                    }
                    vm.run(&func, positional, named, host::emit)
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
    iter: host::ValueIterator,
}

#[pymethods]
impl ArgIter {
    fn __iter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        match self.iter.blocking_read() {
            Ok(c) => Ok(Some(
                cbor_to_python(py, &c)
                    .map_err(|_| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))?
                    .into(),
            )),
            Err(StreamError::Closed) => Ok(None),
            Err(StreamError::LastOperationFailed(e)) => Err(PyErr::new::<
                pyo3::exceptions::PyTypeError,
                _,
            >(e.to_debug_string())),
        }
    }

    fn __aiter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, PyAny>> {
        static AITER: GILOnceCell<PyObject> = GILOnceCell::new();
        AITER
            .import(slf.py(), "promptkit.asyncio", "_aiter_arg")?
            .call1((slf,))
    }

    fn read(&self, py: Python<'_>) -> PyResult<(bool, Option<PyObject>, Option<PyPollable>)> {
        match self.iter.read() {
            Some(Ok(c)) => Ok((
                true,
                Some(
                    cbor_to_python(py, &c)
                        .map_err(|_| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))?
                        .into_pyobject(py)
                        .map_err(|_| PyErr::new::<pyo3::exceptions::PyTypeError, _>("pyo3 error"))?
                        .into(),
                ),
                None,
            )),
            Some(Err(StreamError::Closed)) => Ok((false, None, None)),
            Some(Err(StreamError::LastOperationFailed(e))) => {
                Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    e.to_debug_string(),
                ))
            }
            None => Ok((true, None, Some(PyPollable::from(self.iter.subscribe())))),
        }
    }
}

thread_local! {
    static GLOBAL_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

impl std::io::Write for wasi::io::streams::OutputStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = loop {
            match self.check_write().map(std::num::NonZeroU64::new) {
                Ok(Some(n)) => {
                    break n;
                }
                Ok(None) => {
                    self.subscribe().block();
                }
                Err(StreamError::Closed) => return Ok(0),
                Err(StreamError::LastOperationFailed(e)) => {
                    return Err(std::io::Error::other(e.to_debug_string()));
                }
            }
        };
        let n = n.get().try_into().map_err(std::io::Error::other)?;
        let n = buf.len().min(n);
        wasi::io::streams::OutputStream::write(self, &buf[..n]).map_err(|e| match e {
            StreamError::Closed => std::io::ErrorKind::UnexpectedEof.into(),
            StreamError::LastOperationFailed(e) => std::io::Error::other(e.to_debug_string()),
        })?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.blocking_flush().map_err(std::io::Error::other)
    }
}
