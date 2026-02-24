use thiserror::Error;

use crate::wasm::exports::{self, isola::script::runtime::ErrorCode};

#[derive(Error, Debug)]
pub enum Error {
    #[error("JS error: {cause}")]
    JsError {
        cause: String,
        stack: Option<String>,
    },

    #[error("Unexpected error: {0}")]
    UnexpectedError(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;

impl Error {
    pub fn from_js_catch(ctx: &rquickjs::Ctx<'_>) -> Self {
        let caught = ctx.catch();
        caught.as_exception().map_or_else(
            || Self::JsError {
                cause: format!("{caught:?}"),
                stack: None,
            },
            |exc| Self::JsError {
                cause: exc.message().unwrap_or_default(),
                stack: exc.stack(),
            },
        )
    }
}

impl From<Error> for exports::isola::script::runtime::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::JsError {
                cause,
                stack: Some(stack),
            } => Self {
                code: ErrorCode::Aborted,
                message: format!("{cause}\n\n{stack}"),
            },
            Error::JsError { cause, stack: None } => Self {
                code: ErrorCode::Aborted,
                message: cause,
            },
            Error::UnexpectedError(e) => Self {
                code: ErrorCode::Internal,
                message: e.to_string(),
            },
        }
    }
}
