use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Python error: {0}")]
    PythonError(String),

    #[error("Unexpected error: {0}")]
    UnexpectedError(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;
