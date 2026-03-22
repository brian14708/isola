use napi::Status;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Stream is full")]
    StreamFull,

    #[error("Stream is closed")]
    StreamClosed,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for napi::Error {
    fn from(err: Error) -> Self {
        match err {
            Error::InvalidArgument(msg) => Self::new(Status::InvalidArg, msg),
            Error::Internal(msg) => Self::new(Status::GenericFailure, msg),
            Error::StreamFull => Self::new(Status::GenericFailure, "Stream is full"),
            Error::StreamClosed => Self::new(Status::GenericFailure, "Stream is closed"),
        }
    }
}

pub fn invalid_argument(msg: impl Into<String>) -> Error {
    Error::InvalidArgument(msg.into())
}
