use std::cell::RefCell;

use pyo3::{
    prelude::*,
    types::{PyDict, PyString, PyTuple},
};

use crate::{serde::PyLogDict, wasm::promptkit::vm::host};

thread_local! {
     static GLOBAL_LOGGING: RefCell<i32> = const { RefCell::new(0) };
}

#[pymodule]
#[pyo3(name = "logging")]
pub fn logging_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn debug(
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host::LogLevel::Debug) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host::emit_log(host::LogLevel::Debug, &m);
            }
            Ok(())
        })
    }

    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn info(
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host::LogLevel::Info) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host::emit_log(host::LogLevel::Info, &m);
            }
            Ok(())
        })
    }

    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn warning(
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host::LogLevel::Warn) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host::emit_log(host::LogLevel::Warn, &m);
            }
            Ok(())
        })
    }

    #[pyfn(module)]
    #[pyo3(signature = (msg, *args, **kwds))]
    fn error(
        msg: &Bound<'_, PyString>,
        args: &Bound<'_, PyTuple>,
        kwds: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        GLOBAL_LOGGING.with_borrow(|l| {
            if *l <= loglevel_to_i32(host::LogLevel::Error) {
                let msg = if args.len() > 0 {
                    msg.call_method("format", args, None)?
                } else {
                    msg.clone().into_any()
                };
                let m = PyLogDict::to_json(kwds, msg)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                host::emit_log(host::LogLevel::Error, &m);
            }
            Ok(())
        })
    }

    Ok(())
}

const fn loglevel_to_i32(level: host::LogLevel) -> i32 {
    match level {
        host::LogLevel::Debug => -4,
        host::LogLevel::Info => -3,
        host::LogLevel::Warn => -2,
        host::LogLevel::Error => -1,
    }
}

pub fn set_log_level(level: Option<host::LogLevel>) {
    GLOBAL_LOGGING.with_borrow_mut(|l| match level {
        Some(level) => *l = loglevel_to_i32(level),
        None => *l = 0,
    });
}
