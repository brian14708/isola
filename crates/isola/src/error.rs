use crate::host::BoxError;
use thiserror::Error;

pub type Result<T, E = Error> = core::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestErrorCode {
    Unknown,
    Internal,
    Aborted,
}

impl GuestErrorCode {
    pub(crate) const fn from_wit(code: crate::internal::sandbox::exports::ErrorCode) -> Self {
        match code {
            crate::internal::sandbox::exports::ErrorCode::Unknown => Self::Unknown,
            crate::internal::sandbox::exports::ErrorCode::Internal => Self::Internal,
            crate::internal::sandbox::exports::ErrorCode::Aborted => Self::Aborted,
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    /// Guest-side error from eval/call (maps from WIT `error-code`).
    #[error("[{code:?}] {message}")]
    Guest {
        code: GuestErrorCode,
        message: String,
    },

    /// Network policy denied the request.
    #[error("network denied: {url}: {reason}")]
    NetworkDenied { url: String, reason: String },

    /// Redirect limit exceeded.
    #[error("redirect limit exceeded: {url}")]
    RedirectLimit { url: String },

    /// Wasmtime engine error (instantiation, trap, epoch interrupt).
    #[error("wasm error: {0}")]
    Wasm(#[source] anyhow::Error),

    /// Filesystem I/O error (cache, preopens).
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Embedder-supplied host returned an error (type-erased).
    #[error("host error: {0}")]
    Host(#[source] BoxError),
}

impl From<crate::internal::sandbox::exports::Error> for Error {
    fn from(value: crate::internal::sandbox::exports::Error) -> Self {
        Self::Guest {
            code: GuestErrorCode::from_wit(value.code),
            message: value.message,
        }
    }
}
