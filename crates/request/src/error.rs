#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(#[source] Box<tokio_tungstenite::tungstenite::Error>),

    #[error("URL parsing error: {0}")]
    Url(#[from] url::ParseError),

    #[error("Request body error: {0}")]
    RequestBody(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Internal error: {0}")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::WebSocket(Box::new(err))
    }
}
