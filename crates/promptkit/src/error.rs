use rustpython_vm::builtins::PyBaseExceptionRef;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Python error: {0}")]
    PythonError(String),
}

impl From<PyBaseExceptionRef> for Error {
    fn from(err: PyBaseExceptionRef) -> Self {
        Self::PythonError(format!("{:?}", err))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
