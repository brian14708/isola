use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

pub enum Error {
    Boxed(anyhow::Error),
}

#[derive(serde::Serialize)]
struct ErrorResponse {
    message: String,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Boxed(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    message: err.root_cause().to_string(),
                }),
            )
                .into_response(),
        }
    }
}

impl<E> From<E> for Error
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self::Boxed(err.into())
    }
}

pub type Result<T = Response> = std::result::Result<T, Error>;
