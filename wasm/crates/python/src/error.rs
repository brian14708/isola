use pyo3::{prelude::PyTracebackMethods, PyErr, Python};
use thiserror::Error;

use crate::wasm::exports::{
    self,
    promptkit::script::guest::{self, ErrorCode},
};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Python error: {cause}")]
    PythonError {
        cause: String,
        traceback: Option<String>,
    },

    #[error("Unexpected error: {0}")]
    UnexpectedError(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;

impl Error {
    pub fn from_pyerr(py: Python<'_>, e: impl Into<PyErr>) -> Self {
        let e = e.into();
        Error::PythonError {
            cause: e.to_string(),
            traceback: e.traceback_bound(py).and_then(|e| e.format().ok()),
        }
    }
}

impl From<Error> for exports::promptkit::script::guest::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::PythonError {
                cause,
                traceback: Some(traceback),
            } => guest::Error {
                code: ErrorCode::Aborted,
                message: format!("{cause}\n\n{traceback}"),
            },
            Error::PythonError {
                cause,
                traceback: None,
            } => guest::Error {
                code: ErrorCode::Aborted,
                message: cause,
            },
            Error::UnexpectedError(e) => guest::Error {
                code: ErrorCode::Internal,
                message: e.to_string(),
            },
        }
    }
}
