use thiserror::Error;

pub use crate::vm::exports::ErrorCode;

#[derive(Error, Debug)]
pub enum Error {
    #[error("[{}] {1}", error_code_to_string(*.0))]
    ExecutionError(ErrorCode, String),
}

fn error_code_to_string(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Unknown => "UNKNOWN",
        ErrorCode::Internal => "INTERNAL",
        ErrorCode::Aborted => "ABORTED",
    }
}
