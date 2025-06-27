use std::{
    borrow::Cow,
    cell::RefCell,
    ffi::{CStr, CString, c_char},
};

thread_local! {
    static LAST_ERROR: RefCell<Option<Error>> = const { RefCell::new(None) };
}

#[unsafe(no_mangle)]
pub extern "C" fn promptkit_last_error() -> *const c_char {
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

    #[error("C Error")]
    C(ErrorCode, Cow<'static, CStr>),
}

pub type Result<T> = std::result::Result<T, Error>;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Ok = 0,
    InvalidArgument = 1,
    Internal = 2,
}

trait IntoCStr {
    fn into_cstr(self) -> Cow<'static, CStr>;
}

impl IntoCStr for String {
    fn into_cstr(self) -> Cow<'static, CStr> {
        match CString::new(self) {
            Ok(cstr) => cstr.into(),
            Err(_) => c"invalid utf-8 error string".into(),
        }
    }
}
impl IntoCStr for &'static str {
    fn into_cstr(self) -> Cow<'static, CStr> {
        match CString::new(self) {
            Ok(cstr) => cstr.into(),
            Err(_) => c"invalid utf-8 error string".into(),
        }
    }
}

impl Error {
    fn c_error(&mut self) -> *const c_char {
        match self {
            Error::InvalidArgument(msg) => {
                let cstr = msg.into_cstr();
                *self = Error::C(ErrorCode::InvalidArgument, cstr.clone());
                cstr.as_ptr()
            }
            Error::Internal(msg) => {
                let cstr = msg.clone().into_cstr();
                *self = Error::C(ErrorCode::Internal, cstr.clone());
                cstr.as_ptr()
            }
            Error::C(_, msg) => msg.as_ptr(),
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
            Error::InvalidArgument(_) => ErrorCode::InvalidArgument,
            Error::Internal(_) => ErrorCode::Internal,
            Error::C(code, _) => *code,
        }
    }
}

#[macro_export]
macro_rules! c_try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(e) => {
                let code = $crate::error::ErrorCode::from(&e);
                $crate::error::set_last_error(e);
                return code;
            }
        }
    };
}
