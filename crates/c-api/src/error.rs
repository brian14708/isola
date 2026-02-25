use std::{
    borrow::Cow,
    cell::RefCell,
    ffi::{CStr, CString, c_char},
};

thread_local! {
    static LAST_ERROR: RefCell<Option<Error>> = const { RefCell::new(None) };
}

#[unsafe(no_mangle)]
pub extern "C" fn isola_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map_or(std::ptr::null(), Error::c_error)
    })
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid argument: {0}")]
    InvalidArgument(&'static str),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Stream is full")]
    StreamFull,

    #[error("Stream is closed")]
    StreamClosed,

    #[error("C Error")]
    C(ErrorCode, Cow<'static, CStr>),
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Ok = 0,
    InvalidArgument = 1,
    Internal = 2,
    StreamFull = 3,
    StreamClosed = 4,
}

trait IntoCStr {
    fn into_cstr(self) -> Cow<'static, CStr>;
}

impl IntoCStr for String {
    fn into_cstr(self) -> Cow<'static, CStr> {
        CString::new(self).map_or_else(
            |_| c"invalid utf-8 error string".into(),
            std::convert::Into::into,
        )
    }
}
impl IntoCStr for &'static str {
    fn into_cstr(self) -> Cow<'static, CStr> {
        CString::new(self).map_or_else(
            |_| c"invalid utf-8 error string".into(),
            std::convert::Into::into,
        )
    }
}

impl Error {
    fn c_error(&mut self) -> *const c_char {
        match self {
            Self::InvalidArgument(msg) => {
                let cstr = msg.into_cstr();
                *self = Self::C(ErrorCode::InvalidArgument, cstr);
                self.c_error()
            }
            Self::Internal(msg) => {
                let cstr = msg.clone().into_cstr();
                *self = Self::C(ErrorCode::Internal, cstr);
                self.c_error()
            }
            Self::StreamFull => {
                *self = Self::C(ErrorCode::StreamFull, c"Stream is full".into());
                self.c_error()
            }
            Self::StreamClosed => {
                *self = Self::C(ErrorCode::StreamClosed, c"Stream is closed".into());
                self.c_error()
            }
            Self::C(_, msg) => msg.as_ptr(),
        }
    }
}

pub fn set_last_error(err: Error) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(err);
    });
}

impl From<&Error> for ErrorCode {
    fn from(result: &Error) -> Self {
        match &result {
            Error::InvalidArgument(_) => Self::InvalidArgument,
            Error::Internal(_) => Self::Internal,
            Error::StreamFull => Self::StreamFull,
            Error::StreamClosed => Self::StreamClosed,
            Error::C(code, _) => *code,
        }
    }
}
