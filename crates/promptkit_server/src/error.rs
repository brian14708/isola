use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

pub enum ApiError {
    Boxed(anyhow::Error),
}

#[derive(serde::Serialize)]
struct ErrorResponse {
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Boxed(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    message: err.root_cause().to_string(),
                }),
            )
                .into_response(),
        }
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self::Boxed(err.into())
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
