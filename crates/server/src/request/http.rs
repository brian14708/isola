use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use http::header::HOST;
use http_body::Frame;
use opentelemetry_semantic_conventions::attribute as trace;
use pin_project_lite::pin_project;
use tracing::Span;

use super::Error;

pub async fn http_impl(
    client: reqwest::Client,
    mut request: http::Request<Bytes>,
) -> Result<http::Response<impl Stream<Item = Result<Frame<Bytes>, Error>>>, Error> {
    // Host contract: drop caller-supplied `Host` and let the HTTP client set it.
    request.headers_mut().remove(HOST);
    let url = url::Url::parse(&request.uri().to_string())?;
    let (parts, body) = request.into_parts();
    let r = client
        .request(parts.method, url)
        .version(parts.version)
        .headers(parts.headers)
        .body(body);
    let span = Span::current();
    let mut error_recorded = false;
    let mut resp = match r.send().await {
        Ok(r) => {
            let status = r.status();
            span.record(trace::HTTP_RESPONSE_STATUS_CODE, status.as_u16());
            if status.is_server_error() || status.is_client_error() {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
                error_recorded = true;
            }
            r
        }
        Err(e) => {
            span.record(trace::OTEL_STATUS_CODE, "ERROR");
            return Err(Error::Http(e));
        }
    };

    let mut builder = http::response::Builder::new()
        .status(resp.status())
        .version(resp.version());
    if let Some(h) = builder.headers_mut() {
        *h = std::mem::take(resp.headers_mut());
    }
    let b = InstrumentStream::new(
        span,
        error_recorded,
        resp.bytes_stream().map(|f| match f {
            Ok(d) => Ok(Frame::data(d)),
            Err(e) => Err(e.into()),
        }),
    );
    builder.body(b).map_err(|e| Error::Internal(Box::new(e)))
}

pin_project! {
    struct InstrumentStream<S> {
        #[pin]
        stream: S,
        span: tracing::Span,
        size: usize,
        error_recorded: bool,
    }
}

impl<S> InstrumentStream<S> {
    const fn new(span: Span, error_recorded: bool, stream: S) -> Self {
        Self {
            stream,
            span,
            size: 0,
            error_recorded,
        }
    }
}

impl<S: Stream<Item = Result<http_body::Frame<Bytes>, E>>, E> Stream for InstrumentStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let span = &this.span;
        let enter = span.enter();
        match this.stream.poll_next(cx) {
            Poll::Ready(None) => {
                if !*this.error_recorded {
                    span.record(trace::OTEL_STATUS_CODE, "OK");
                }
                span.record(trace::HTTP_RESPONSE_BODY_SIZE, *this.size as u64);
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(f))) => {
                if let Some(d) = f.data_ref() {
                    *this.size += d.len();
                }
                Poll::Ready(Some(Ok(f)))
            }
            Poll::Ready(Some(Err(e))) => {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
                *this.error_recorded = true;
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
