use thiserror::Error;

pub use crate::vm::exports::ErrorCode;

#[derive(Error, Debug)]
pub enum Error {
    #[error("[{code}] {1}", code = error_code_to_string(*.0))]
    ExecutionError(ErrorCode, String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<crate::vm::exports::Error> for Error {
    fn from(value: crate::vm::exports::Error) -> Self {
        Self::ExecutionError(value.code, value.message)
    }
}

fn error_code_to_string(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Unknown => "UNKNOWN",
        ErrorCode::Internal => "INTERNAL",
        ErrorCode::Aborted => "ABORTED",
    }
}

pub type Result<T, E = Error> = core::result::Result<T, E>;
