use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use isola::sandbox::Error as IsolaError;

use super::types::{ErrorCode, ErrorResponse, HttpError};

#[derive(Debug)]
pub struct HttpApiError {
    pub code: ErrorCode,
    pub message: String,
}

impl HttpApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    pub fn unknown_runtime(runtime: &str) -> Self {
        Self::new(
            ErrorCode::UnknownRuntime,
            format!("Unknown runtime: {runtime}"),
        )
    }

    pub fn script_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ScriptError, message)
    }

    pub fn timeout(timeout_ms: u64) -> Self {
        Self::new(
            ErrorCode::Timeout,
            format!("Execution timed out after {timeout_ms}ms"),
        )
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl IntoResponse for HttpApiError {
    fn into_response(self) -> Response {
        let status = match self.code {
            ErrorCode::InvalidRequest | ErrorCode::UnknownRuntime => StatusCode::BAD_REQUEST,
            ErrorCode::ScriptError => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Timeout => StatusCode::REQUEST_TIMEOUT,
            ErrorCode::Cancelled => {
                StatusCode::from_u16(499).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            }
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = ErrorResponse {
            error: HttpError {
                code: self.code,
                message: self.message,
            },
        };

        (status, Json(body)).into_response()
    }
}

impl From<IsolaError> for HttpApiError {
    fn from(err: IsolaError) -> Self {
        match err {
            IsolaError::Guest { message } => Self::script_error(message),
            IsolaError::Runtime(err) => {
                let message = err.to_string();
                if message.contains("interrupt") {
                    Self::new(ErrorCode::Timeout, "Execution timed out")
                } else {
                    Self::internal(message)
                }
            }
        }
    }
}

impl std::fmt::Display for HttpApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for HttpApiError {}
