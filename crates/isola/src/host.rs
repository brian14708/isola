use bytes::Bytes;
use http_body::Frame;
#[cfg(feature = "request")]
use std::sync::OnceLock;
use std::{pin::Pin, sync::Arc};

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type BoxedStream<T> = Pin<Box<dyn futures::Stream<Item = T> + Send>>;

pub type HttpBodyStream = BoxedStream<core::result::Result<Frame<Bytes>, BoxError>>;
pub type HttpRequest = http::Request<Option<Bytes>>;
pub type HttpResponse = http::Response<HttpBodyStream>;

#[async_trait::async_trait]
pub trait OutputSink: Send + 'static {
    async fn on_partial(&mut self, cbor: Bytes) -> core::result::Result<(), BoxError>;
    async fn on_end(&mut self, cbor: Bytes) -> core::result::Result<(), BoxError>;
}

#[async_trait::async_trait]
pub trait Host: Send + Sync + 'static {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Bytes,
    ) -> core::result::Result<Bytes, BoxError> {
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
        #[cfg(feature = "request")]
        {
            use crate::request::{Client, RequestOptions};
            use futures::StreamExt as _;

            static DEFAULT_CLIENT: OnceLock<Client> = OnceLock::new();
            let client = DEFAULT_CLIENT.get_or_init(Client::new);

            let (parts, body) = req.into_parts();
            let mut request = http::Request::new(body.unwrap_or_default());
            *request.method_mut() = parts.method;
            *request.uri_mut() = parts.uri;
            *request.headers_mut() = parts.headers;

            let response = client
                .send_http(request, RequestOptions::default())
                .await
                .map_err(|e| -> BoxError { Box::new(e) })?;

            Ok(response.map(|body| -> HttpBodyStream {
                Box::pin(body.map(|frame| frame.map_err(|e| -> BoxError { Box::new(e) })))
            }))
        }

        #[cfg(not(feature = "request"))]
        {
            let _req = req;
            Err(
                std::io::Error::new(std::io::ErrorKind::Unsupported, "unsupported http_request")
                    .into(),
            )
        }
    }
}

#[async_trait::async_trait]
impl<T: Host + ?Sized> Host for Arc<T> {
    async fn hostcall(
        &self,
        call_type: &str,
        payload: Bytes,
    ) -> core::result::Result<Bytes, BoxError> {
        (**self).hostcall(call_type, payload).await
    }

    async fn http_request(&self, req: HttpRequest) -> core::result::Result<HttpResponse, BoxError> {
        (**self).http_request(req).await
    }
}
