use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{Stream, StreamExt, TryFutureExt};
use http_body_util::BodyExt;
use opentelemetry_semantic_conventions::attribute as trace;
use pin_project_lite::pin_project;
use tokio_tungstenite::tungstenite;
use tracing::{Instrument, Span};

use crate::{Error, WebsocketMessage};

pub async fn http_impl<B>(
    span: Span,
    client: Result<reqwest::Client, Error>,
    mut request: http::Request<B>,
) -> Result<
    http::Response<
        impl Stream<Item = Result<http_body::Frame<Bytes>, Error>> + Send + Sync + 'static,
    >,
    Error,
>
where
    B: http_body::Body + Send + Sync + 'static,
    B::Error: std::error::Error + Send + Sync,
    B::Data: Send,
{
    let r = client?
        .request(
            std::mem::take(request.method_mut()),
            reqwest::Url::parse(request.uri().to_string().as_str())?,
        )
        .version(request.version())
        .headers(std::mem::take(request.headers_mut()))
        .body(request.into_body().collect().await?.to_bytes());
    let mut resp = match r.send().instrument(span.clone()).await.map_err(Box::new) {
        Ok(r) => {
            let status = r.status();
            span.record(trace::HTTP_RESPONSE_STATUS_CODE, status.as_u16());
            if status.is_server_error() || status.is_client_error() {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
            }
            r
        }
        Err(e) => {
            span.record(trace::OTEL_STATUS_CODE, "ERROR");
            return Err(e);
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
        resp.bytes_stream().map(|f| match f {
            Ok(d) => Ok(http_body::Frame::data(d)),
            Err(e) => Err(e.into()),
        }),
    );
    Ok(builder.body(b)?)
}

pub async fn websocket_impl(
    span: Span,
    client: reqwest::Client,
    mut request: http::Request<impl Stream<Item = WebsocketMessage> + Send + 'static>,
) -> Result<
    http::Response<impl Stream<Item = Result<WebsocketMessage, Error>> + Send + 'static>,
    Error,
> {
    let r = client
        .request(
            std::mem::take(request.method_mut()),
            reqwest::Url::parse(request.uri().to_string().as_str())?,
        )
        .version(request.version())
        .headers(std::mem::take(request.headers_mut()));

    let conn = match r
        .send()
        .and_then(|resp| async {
            span.record(trace::HTTP_RESPONSE_STATUS_CODE, resp.status().as_u16());
            resp.upgrade().await
        })
        .and_then(|response| async {
            Ok(tokio_tungstenite::WebSocketStream::from_raw_socket(
                response,
                tungstenite::protocol::Role::Client,
                None,
            )
            .await)
        })
        .instrument(span.clone())
        .await
        .map_err(Box::new)
    {
        Ok(r) => r,
        Err(e) => {
            span.record(trace::OTEL_STATUS_CODE, "ERROR");
            return Err(e);
        }
    };

    let (tx, rx) = conn.split();
    let write = request
        .into_body()
        .map(Ok)
        .forward(tx)
        .map_err(|e| -> Error { Box::new(e) });
    let builder = http::response::Builder::new();
    Ok(builder.body(InstrumentJoinStream::new(
        span,
        rx.map(|f| match f {
            Ok(m) => Ok(m),
            Err(e) => Err(e.into()),
        }),
        write,
    ))?)
}

pin_project! {
    struct InstrumentStream<S> {
        #[pin]
        stream: S,
        span: tracing::Span,
        size: usize,
    }
}

impl<S> InstrumentStream<S> {
    fn new(span: Span, stream: S) -> Self {
        InstrumentStream {
            stream,
            span,
            size: 0,
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
                span.record(trace::OTEL_STATUS_CODE, "OK");
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
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pin_project! {
    struct InstrumentJoinStream<S, F> {
        #[pin]
        stream: Option<S>,
        #[pin]
        fut: Option<F>,
        span: tracing::Span,
    }
}

impl<S, F> InstrumentJoinStream<S, F> {
    fn new(span: Span, stream: S, fut: F) -> Self {
        Self {
            stream: Some(stream),
            span,
            fut: Some(fut),
        }
    }
}

impl<S: Stream<Item = Result<M, E>>, F: Future<Output = Result<(), E>>, M, E> Stream
    for InstrumentJoinStream<S, F>
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let span = &this.span;
        let enter = span.enter();
        if let Some(stream) = this.stream.as_mut().as_pin_mut() {
            match stream.poll_next(cx) {
                Poll::Ready(None) => this.stream.set(None),
                v @ Poll::Ready(Some(_)) => return v,
                Poll::Pending => {}
            }
        }

        if let Some(task) = this.fut.as_mut().as_pin_mut() {
            match task.poll(cx) {
                Poll::Ready(Ok(())) => this.fut.set(None),
                Poll::Ready(Err(err)) => {
                    span.record(trace::OTEL_STATUS_CODE, "ERROR");
                    drop(enter);
                    this.stream.set(None);
                    this.fut.set(None);
                    *this.span = tracing::Span::none();
                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if this.stream.is_none() && this.fut.is_none() {
            span.record(trace::OTEL_STATUS_CODE, "OK");
            drop(enter);
            *this.span = tracing::Span::none();
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}
