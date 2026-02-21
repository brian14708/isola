use std::{pin::Pin, sync::Arc};

use bytes::Bytes;
use http_body::Frame;

use crate::value::Value;

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type BoxedStream<T> = Pin<Box<dyn futures::Stream<Item = T> + Send>>;

pub type HttpBodyStream = BoxedStream<core::result::Result<Frame<Bytes>, BoxError>>;
pub type HttpRequest = http::Request<Option<Bytes>>;
pub type HttpResponse = http::Response<HttpBodyStream>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
    Stdout,
    Stderr,
}

impl LogLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Critical => "critical",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

impl From<&str> for LogLevel {
    fn from(context: &str) -> Self {
        match context {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "warn" => Self::Warn,
            "error" => Self::Error,
            "critical" => Self::Critical,
            "stdout" => Self::Stdout,
            "stderr" => Self::Stderr,
            _ => Self::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogContext<'a> {
    Stdout,
    Stderr,
    Other(&'a str),
}

#[async_trait::async_trait]
pub trait OutputSink: Send + Sync + 'static {
    async fn on_item(&self, value: Value) -> core::result::Result<(), BoxError>;
    async fn on_complete(&self, value: Option<Value>) -> core::result::Result<(), BoxError>;

    /// # Errors
    /// Returns an error to fail the current guest execution when log delivery
    /// fails.
    async fn on_log(
        &self,
        _level: LogLevel,
        _log_context: LogContext<'_>,
        _message: &str,
    ) -> core::result::Result<(), BoxError> {
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopOutputSink;

static SHARED_NOOP_OUTPUT_SINK: std::sync::LazyLock<Arc<NoopOutputSink>> =
    std::sync::LazyLock::new(|| Arc::new(NoopOutputSink));

impl NoopOutputSink {
    #[must_use]
    pub fn shared() -> Arc<dyn OutputSink> {
        SHARED_NOOP_OUTPUT_SINK.clone()
    }
}

#[async_trait::async_trait]
impl OutputSink for NoopOutputSink {
    async fn on_item(&self, _value: Value) -> core::result::Result<(), BoxError> {
        Ok(())
    }

    async fn on_complete(&self, _value: Option<Value>) -> core::result::Result<(), BoxError> {
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait Host: Send + Sync + 'static {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Value,
    ) -> core::result::Result<Value, BoxError> {
        let _payload = payload;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("unsupported hostcall: {call_type}"),
        )
        .into())
    }

    /// Perform an HTTP request.
    ///
    /// Implementations own redirect behavior and header hygiene. In particular,
    /// remove any caller-supplied `Host` header before dispatching.
    async fn http_request(&self, req: HttpRequest) -> core::result::Result<HttpResponse, BoxError> {
        let _req = req;
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "unsupported http_request").into())
    }
}

#[async_trait::async_trait]
impl<T: Host + ?Sized> Host for Arc<T> {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Value,
    ) -> core::result::Result<Value, BoxError> {
        (**self).hostcall(call_type, payload).await
    }

    async fn http_request(&self, req: HttpRequest) -> core::result::Result<HttpResponse, BoxError> {
        (**self).http_request(req).await
    }
}
