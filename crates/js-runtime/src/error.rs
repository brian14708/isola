use thiserror::Error;

use crate::wasm::exports::{self, isola::script::runtime::ErrorCode};

#[derive(Error, Debug)]
pub enum Error {
    #[error("JS error: {cause}")]
    Js {
        cause: String,
        stack: Option<String>,
    },

    #[error("{0}")]
    Transpile(String),

    #[error("Unexpected error: {0}")]
    Unexpected(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;

impl Error {
    pub fn from_js_catch(ctx: &rquickjs::Ctx<'_>) -> Self {
        let caught = ctx.catch();
        caught.as_exception().map_or_else(
            || Self::Js {
                cause: format!("{caught:?}"),
                stack: None,
            },
            |exc| Self::Js {
                cause: exc.message().unwrap_or_default(),
                stack: exc.stack(),
            },
        )
    }
}

impl From<Error> for exports::isola::script::runtime::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::Js {
                cause,
                stack: Some(stack),
            } => Self {
                code: ErrorCode::Aborted,
                message: format!("{cause}\n\n{stack}"),
            },
            Error::Js { cause, stack: None } => Self {
                code: ErrorCode::Aborted,
                message: cause,
            },
            Error::Transpile(message) => Self {
                code: ErrorCode::Aborted,
                message,
            },
            Error::Unexpected(e) => Self {
                code: ErrorCode::Internal,
                message: e.to_string(),
            },
        }
    }
}
