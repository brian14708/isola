#![expect(clippy::same_length_and_capacity)]

mod body_buffer;
mod future;
mod http;
mod logging;
mod serde;

use std::cell::RefCell;

use self::wasi::io::streams::StreamError;
use self::wasi::logging::logging::Level;
use pyo3::{append_to_inittab, prelude::*, sync::PyOnceLock};

use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde::cbor_to_python,
    wasm::future::PyPollable,
};

use self::{exports::isola::script::guest, isola::script::host};

wit_bindgen::generate!({
    world: "sandbox",
    path: "../isola/wit",
    generate_all,
});

#[pymodule]
#[pyo3(name = "_isola_sys")]
pub mod sys_module {
    use std::{ops::Deref, time::Duration};

    use pyo3::{
        Bound, PyAny, PyErr, PyResult, Python, pyfunction,
        types::{PyBytes, PyList, PyListMethods, PyTuple, PyTupleMethods},
    };
    use smallvec::{SmallVec, smallvec};

    use crate::{
        serde::{cbor_to_python, python_to_cbor, python_to_cbor_emit},
        wasm::{
            future::{Pollable, create_future},
            isola::script::host,
            wasi,
        },
    };

    use super::wasi::{
        clocks::monotonic_clock::{now, subscribe_duration},
        io::poll::poll as host_poll,
    };

    #[pymodule_export]
    use super::future::PyPollable;

    fn cbor_convert(
        py: Python<'_>,
        cbor: Result<Vec<u8>, wasi::io::error::Error>,
    ) -> PyResult<Bound<'_, PyAny>> {
        cbor_to_python(
            py,
            &cbor
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_debug_string()))?,
        )
    }
    create_future!(PyFutureHostcall, super::host::FutureHostcall, cbor_convert -> PyResult<Bound<'_, PyAny>>);

    #[pyfunction]
    #[pyo3(signature = (duration))]
    fn sleep(duration: f64) -> PyPollable {
        subscribe_duration(
            u64::try_from(Duration::from_secs_f64(duration).as_nanos())
                .expect("duration is too large"),
        )
        .into()
    }

    #[pyfunction]
    fn monotonic() -> f64 {
        Duration::from_nanos(now()).as_secs_f64()
    }

    #[pyfunction]
    fn emit(obj: Bound<'_, PyAny>) -> PyResult<()> {
        python_to_cbor_emit(obj, host::EmitType::PartialResult, host::blocking_emit)
    }

    #[pyfunction]
    fn hostcall(call_type: &str, payload: Bound<'_, PyAny>) -> PyResult<PyFutureHostcall> {
        let cbor_payload = python_to_cbor(payload)?;
        let future_hostcall = host::hostcall(call_type, &cbor_payload);
        Ok(PyFutureHostcall::new(future_hostcall))
    }

    #[pyfunction]
    #[pyo3(signature = (poll))]
    fn poll<'py>(poll: &Bound<'py, PyList>) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let (pollables, ready_count) = poll.iter().try_fold(
            (SmallVec::<[_; 8]>::with_capacity(poll.len()), 0),
            |(mut vec, mut count), p| -> PyResult<_> {
                let pollable =
                    Pollable::subscribe(p.cast_exact::<PyTuple>()?.get_borrowed_item(0)?)?;
                if pollable.is_none() {
                    count += 1;
                }
                vec.push(pollable);
                Ok((vec, count))
            },
        )?;
        assert!(pollables.len() == poll.len());

        let py = poll.py();
        if ready_count > 0 {
            Ok(Some(PyBytes::new(
                py,
                &pollables
                    .into_iter()
                    .map(|p| u8::from(p.is_none()))
                    .collect::<SmallVec<[_; 8]>>(),
            )))
        } else {
            let handles = pollables
                .iter()
                .map(|p| p.as_ref().unwrap().deref())
                .collect::<SmallVec<[_; 8]>>();

            let mut result: SmallVec<[_; 8]> = smallvec![0; poll.len()];
            for index in host_poll(&handles) {
                result[index as usize] = 1;
            }
            Ok(Some(PyBytes::new(py, &result)))
        }
    }
}

#[cfg(target_arch = "wasm32")]
export!(Global);

pub struct Global;

impl guest::Guest for Global {
    fn initialize(preinit: bool, prelude: Option<String>) {
        GLOBAL_SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            if scope.is_none() {
                use http::http_module;
                use logging::logging_module;
                use serde::serde_module;

                append_to_inittab!(http_module);
                append_to_inittab!(logging_module);
                append_to_inittab!(sys_module);
                append_to_inittab!(serde_module);

                let v = Scope::new();
                if let Some(prelude) = prelude {
                    v.load_script(&prelude).unwrap();
                    v.flush();
                }
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
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::UnexpectedError("Sandbox not initialized").into()),
                |sandbox| {
                    sandbox
                        .load_script(&script)
                        .map_err(Into::<guest::Error>::into)
                },
            )
        })
    }

    fn eval_file(path: String) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            if let Some(sandbox) = sandbox.as_ref() {
                let script = std::fs::read_to_string(std::path::Path::new(&path))
                    .map_err(|_e| Error::UnexpectedError("fail to read script"))?;
                sandbox
                    .load_script(&script)
                    .map_err(Into::<guest::Error>::into)
            } else {
                Err(Error::UnexpectedError("Sandbox not initialized").into())
            }
        })
    }

    fn call_func(func: String, args: Vec<guest::Argument>) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::UnexpectedError("Sandbox not initialized").into()),
                |sandbox| {
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
                    let ret = sandbox
                        .run(&func, positional, named, |emit_type, data| {
                            host::blocking_emit(emit_type, data);
                        })
                        .map_err(Into::<guest::Error>::into);
                    sandbox.flush();
                    ret
                },
            )
        })
    }
}

#[pyclass]
pub struct ArgIter {
    iter: host::ValueIterator,
}

#[pymethods]
impl ArgIter {
    const fn __iter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
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
        static AITER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
        AITER
            .import(slf.py(), "sandbox.asyncio", "_aiter_arg")?
            .call1((slf,))
    }

    fn read(&self, py: Python<'_>) -> PyResult<(bool, Option<Py<PyAny>>, Option<PyPollable>)> {
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
        Self::write(self, &buf[..n]).map_err(|e| match e {
            StreamError::Closed => std::io::ErrorKind::UnexpectedEof.into(),
            StreamError::LastOperationFailed(e) => std::io::Error::other(e.to_debug_string()),
        })?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.blocking_flush().map_err(std::io::Error::other)
    }
}
