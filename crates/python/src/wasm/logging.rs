use std::cell::RefCell;

use pyo3::prelude::*;

use super::wasi::logging::logging::Level;

thread_local! {
     static GLOBAL_LOGGING: RefCell<i32> = const { RefCell::new(0) };
}

#[pymodule]
#[pyo3(name = "_isola_logging")]
pub mod logging_module {
    use pyo3::{
        Bound, PyAny, PyErr, PyResult, pyfunction,
        types::{PyAnyMethods, PyDict, PyString, PyTuple, PyTupleMethods},
    };

    use crate::{
        serde::python_to_json,
        wasm::wasi::logging::logging::{Level, log},
    };

    use super::{GLOBAL_LOGGING, loglevel_to_i32};

    fn log_dict_to_json(
        dict: Option<&Bound<'_, PyDict>>,
        msg: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let out = PyDict::new(msg.py());
        out.set_item("message", msg)?;
        if let Some(d) = dict {
            for (key, value) in d {
                if value.is_callable() {
                    out.set_item(key, value.call0()?)?;
                } else {
                    out.set_item(key, value)?;
                }
            }
        }
        python_to_json(out.into_any()).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                "Failed to convert log dict to JSON: {e}"
            ))
        })
    }

    #[pyfunction]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn debug(
        msg: &Bound<'_, PyString>,
        args: Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(Level::Debug) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = log_dict_to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                log(Level::Debug, "log", &m);
            }
            Ok(())
        })
    }

    #[pyfunction]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn info(
        msg: &Bound<'_, PyString>,
        args: Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(Level::Info) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = log_dict_to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                log(Level::Info, "log", &m);
            }
            Ok(())
        })
    }

    #[pyfunction]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn warning(
        msg: &Bound<'_, PyString>,
        args: Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(Level::Warn) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = log_dict_to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                log(Level::Warn, "log", &m);
            }
            Ok(())
        })
    }

    #[pyfunction]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn error(
        msg: &Bound<'_, PyString>,
        args: Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(Level::Error) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = log_dict_to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                log(Level::Error, "log", &m);
            }
            Ok(())
        })
    }
}

const fn loglevel_to_i32(level: Level) -> i32 {
    match level {
        Level::Trace | Level::Debug => -4,
        Level::Info => -3,
        Level::Warn => -2,
        Level::Error | Level::Critical => -1,
    }
}

pub fn set_log_level(level: Option<Level>) {
    GLOBAL_LOGGING.with_borrow_mut(|l| match level {
        Some(level) => *l = loglevel_to_i32(level),
        None => *l = 0,
    });
}
