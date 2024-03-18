use pyo3::{PyErr, Python};
use thiserror::Error;

use crate::wasm::exports;

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
            traceback: e.traceback(py).and_then(|e| e.format().ok()),
        }
    }
}

impl From<Error> for exports::promptkit::script::guest_api::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::PythonError {
                cause,
                traceback: Some(traceback),
            } => (4, format!("{cause}\n\n{traceback}")),
            Error::PythonError {
                cause,
                traceback: None,
            } => (4, cause),
            Error::UnexpectedError(e) => (3, e.to_string()),
        }
    }
}
