use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use isola::{
    host::{
        BoxError, Host, HttpBodyStream, HttpRequest, HttpResponse, LogContext, LogLevel, OutputSink,
    },
    value::Value,
};
use tokio::sync::mpsc;
use tracing::field::Empty;

use crate::request::{Client, RequestOptions, TraceRequest};

const SCRIPT_TRACE_TARGET: &str = "isola_server::script";

fn make_request_span(request: &TraceRequest<'_>) -> tracing::Span {
    match request {
        TraceRequest::Http(request) => {
            tracing::span!(
                target: SCRIPT_TRACE_TARGET,
                tracing::Level::INFO,
                "http.request",
                otel.kind = "client",
                { ::opentelemetry_semantic_conventions::attribute::HTTP_REQUEST_METHOD } = request.method.as_str(),
                { ::opentelemetry_semantic_conventions::attribute::SERVER_ADDRESS } = request.uri.host().unwrap_or_default(),
                { ::opentelemetry_semantic_conventions::attribute::SERVER_PORT } = request.uri.port_u16().unwrap_or_else(|| {
                        match request.uri.scheme_str() {
                            Some("http") => 80,
                            Some("https") => 443,
                            _ => 0,
                        }
                    }),
                { ::opentelemetry_semantic_conventions::attribute::URL_FULL } = request.uri.to_string(),
                { ::opentelemetry_semantic_conventions::attribute::HTTP_RESPONSE_STATUS_CODE } = Empty,
                { ::opentelemetry_semantic_conventions::attribute::HTTP_RESPONSE_BODY_SIZE } = Empty,
                { ::opentelemetry_semantic_conventions::attribute::OTEL_STATUS_CODE } = Empty,
            )
        }
    }
}

#[derive(Clone)]
pub struct SandboxEnv {
    pub client: Arc<Client>,
}

pub enum StreamItem {
    Data(Value),
    End(Option<Value>),
    Log {
        level: String,
        context: String,
        message: String,
    },
    Error(isola::sandbox::Error),
}

#[derive(Clone)]
pub struct MpscOutputSink {
    sender: mpsc::UnboundedSender<StreamItem>,
}

impl MpscOutputSink {
    #[must_use]
    pub const fn new(sender: mpsc::UnboundedSender<StreamItem>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl OutputSink for MpscOutputSink {
    async fn on_item(&self, value: Value) -> Result<(), BoxError> {
        self.sender.send(StreamItem::Data(value)).map_err(|_e| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "output receiver dropped",
            )) as BoxError
        })
    }

    async fn on_complete(&self, value: Option<Value>) -> Result<(), BoxError> {
        self.sender.send(StreamItem::End(value)).map_err(|_e| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "output receiver dropped",
            )) as BoxError
        })
    }

    async fn on_log(
        &self,
        level: LogLevel,
        log_context: LogContext<'_>,
        message: &str,
    ) -> Result<(), BoxError> {
        self.sender
            .send(StreamItem::Log {
                level: level.as_str().to_string(),
                context: match log_context {
                    LogContext::Stdout => "stdout".to_string(),
                    LogContext::Stderr => "stderr".to_string(),
                    LogContext::Other(context) => context.to_string(),
                },
                message: message.to_string(),
            })
            .map_err(|_e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "output receiver dropped",
                )) as BoxError
            })
    }
}

#[async_trait]
impl Host for SandboxEnv {
    async fn http_request(&self, incoming: HttpRequest) -> Result<HttpResponse, BoxError> {
        let mut request = http::Request::new(incoming.body().clone().unwrap_or_default());
        *request.method_mut() = incoming.method().clone();
        *request.uri_mut() = incoming.uri().clone();
        *request.headers_mut() = incoming.headers().clone();

        let response = self
            .client
            .send_http(
                request,
                RequestOptions::new().with_make_span(make_request_span),
            )
            .await
            .map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)?;

        Ok(response.map(|body| -> HttpBodyStream {
            Box::pin(
                body.map(|frame| frame.map_err(|e| Box::new(std::io::Error::other(e)) as BoxError)),
            )
        }))
    }
}
