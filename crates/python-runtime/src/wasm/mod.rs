mod body_buffer;
mod future;
mod http;
mod logging;
mod serde;

use std::cell::RefCell;

pub use isola_runtime::{exports, isola, wasi};
use pyo3::{append_to_inittab, prelude::*, sync::PyOnceLock};

use self::{exports::isola::script::runtime, isola::script::host};
use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde::cbor_to_python,
    wasm::future::PyPollable,
};

#[pymodule]
#[pyo3(name = "_isola_sys")]
pub mod sys_module {
    use pyo3::{
        Bound, PyAny, PyErr, PyRef, PyResult, Python, pyfunction,
        types::{PyAnyMethods, PyBytes, PyList, PyListMethods},
    };

    #[pymodule_export]
    use super::future::PyPollable;
    use crate::{
        serde::{cbor_to_python, python_to_cbor, python_to_cbor_emit},
        wasm::{future::create_future, isola::script::host},
    };

    fn cbor_convert(py: Python<'_>, cbor: Result<Vec<u8>, String>) -> PyResult<Bound<'_, PyAny>> {
        cbor_to_python(
            py,
            &cbor.map_err(PyErr::new::<pyo3::exceptions::PyTypeError, _>)?,
        )
    }
    create_future!(PyFutureHostcall, cbor_convert -> PyResult<Bound<'_, PyAny>>);

    fn item_pollable<'py>(item: &Bound<'py, PyAny>) -> Option<PyRef<'py, PyPollable>> {
        item.get_item(0)
            .ok()?
            .extract::<PyRef<'_, PyPollable>>()
            .ok()
    }

    fn ready_set(pollables: &Bound<'_, PyList>) -> Vec<u8> {
        pollables
            .iter()
            .map(|item| item_pollable(&item).is_none_or(|pollable| pollable.is_ready()))
            .map(u8::from)
            .collect::<Vec<_>>()
    }

    #[pyfunction]
    #[pyo3(signature = (duration))]
    fn sleep(duration: f64) -> PyResult<PyPollable> {
        let deadline = isola_runtime::Deadline::after_secs_f64(duration).map_err(|_| {
            pyo3::exceptions::PyOverflowError::new_err("sleep duration is out of range")
        })?;
        Ok(PyPollable::sleep(deadline))
    }

    #[pyfunction]
    fn monotonic() -> f64 {
        isola_runtime::monotonic()
    }

    #[pyfunction]
    fn emit(obj: Bound<'_, PyAny>) -> PyResult<()> {
        python_to_cbor_emit(obj, host::EmitType::PartialResult, host::blocking_emit)
    }

    #[pyfunction]
    fn hostcall(call_type: &str, payload: Bound<'_, PyAny>) -> PyResult<PyFutureHostcall> {
        let cbor_payload = python_to_cbor(payload)?;
        Ok(PyFutureHostcall::new(crate::wasm::future::register_call(
            call_type.to_string(),
            cbor_payload,
        )))
    }

    #[pyfunction]
    fn ready<'py>(poll: &Bound<'py, PyList>) -> Bound<'py, PyBytes> {
        PyBytes::new(poll.py(), &ready_set(poll))
    }

    #[pyfunction]
    #[pyo3(signature = (step, suspend = false))]
    fn drive(step: &Bound<'_, PyAny>, suspend: bool) -> PyResult<()> {
        let mut error = None;
        let _ = crate::wasm::future::drive_pending_calls(|| {
            match step.call0().and_then(|wait| wait.extract::<bool>()) {
                Ok(true) => isola_runtime::pending::Drive::Wait,
                Ok(false) if suspend => isola_runtime::pending::Drive::Suspend,
                Ok(false) => isola_runtime::pending::Drive::Stop,
                Err(cause) => {
                    error = Some(cause);
                    isola_runtime::pending::Drive::Stop
                }
            }
        });
        error.map_or(Ok(()), Err)
    }
}

#[cfg(target_arch = "wasm32")]
isola_runtime::export!(Global with_types_in isola_runtime);

pub struct Global;

impl runtime::Guest for Global {
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
                isola_runtime::pending::clear();
                scope.replace(v);
            }
        });

        // https://github.com/bytecodealliance/componentize-py/blob/72348e0ebd74ef1027c52528409a289765ed5c4c/runtime/src/lib.rs#L377
        if preinit {
            isola_runtime::lifecycle::reset_preinitialized_state();
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "WIT async export requires an async trait method"
    )]
    async fn eval_script(script: String) -> Result<(), runtime::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::UnexpectedError("Sandbox not initialized").into()),
                |sandbox| {
                    let result = sandbox
                        .load_script(&script)
                        .map_err(Into::<runtime::Error>::into);
                    sandbox.flush();
                    isola_runtime::pending::clear();
                    result
                },
            )
        })
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "WIT async export requires an async trait method"
    )]
    async fn eval_file(path: String) -> Result<(), runtime::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            if let Some(sandbox) = sandbox.as_ref() {
                let script = std::fs::read_to_string(std::path::Path::new(&path))
                    .map_err(|_e| Error::UnexpectedError("fail to read script"))?;
                let result = sandbox
                    .load_script(&script)
                    .map_err(Into::<runtime::Error>::into);
                sandbox.flush();
                isola_runtime::pending::clear();
                result
            } else {
                Err(Error::UnexpectedError("Sandbox not initialized").into())
            }
        })
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "WIT async export requires an async trait method"
    )]
    async fn call_func(func: String, args: Vec<runtime::Argument>) -> Result<(), runtime::Error> {
        GLOBAL_SCOPE.with_borrow(|sandbox| {
            sandbox.as_ref().map_or_else(
                || Err(Error::UnexpectedError("Sandbox not initialized").into()),
                |sandbox| {
                    let mut positional = vec![];
                    let mut named = vec![];
                    for arg in args {
                        let runtime::Argument { name, value } = arg;
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
                        .map_err(Into::<runtime::Error>::into);
                    sandbox.flush();
                    isola_runtime::pending::clear();
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
        match isola_runtime::block_on(self.iter.read()) {
            Some(c) => Ok(Some(
                cbor_to_python(py, &c)
                    .map_err(|_| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))?
                    .into(),
            )),
            None => Ok(None),
        }
    }

    fn __aiter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, PyAny>> {
        static AITER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
        AITER
            .import(slf.py(), "sandbox.asyncio", "_aiter_arg")?
            .call1((slf,))
    }

    fn read(&self, py: Python<'_>) -> PyResult<(bool, Option<Py<PyAny>>, Option<PyPollable>)> {
        match isola_runtime::block_on(self.iter.read()) {
            Some(c) => Ok((
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
            None => Ok((false, None, None)),
        }
    }
}

thread_local! {
    static GLOBAL_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}
