use std::{future::Future, pin::Pin, task::Poll, time::Duration};

use axum::{
    http::{header::CONTENT_TYPE, HeaderValue},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
};
use serde_json::json;
use tokio_stream::Stream;

pub struct StreamResponse<S>(pub S);

impl<S> IntoResponse for StreamResponse<S>
where
    S: Stream<Item = anyhow::Result<Event>> + Send + 'static,
{
    fn into_response(self) -> Response {
        let mut response = Sse::new(StreamUntilError::new(self.0))
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(1))
                    .text("keepalive"),
            )
            .into_response();
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );
        response
    }
}

#[pin_project::pin_project]
struct StreamUntilError<S> {
    #[pin]
    stream: Option<S>,
}

impl<S, E> StreamUntilError<S>
where
    S: Stream<Item = anyhow::Result<E>> + Send + 'static,
{
    const fn new(stream: S) -> Self {
        Self {
            stream: Some(stream),
        }
    }
}

impl<S> Stream for StreamUntilError<S>
where
    S: Stream<Item = anyhow::Result<Event>> + Send + 'static,
{
    type Item = anyhow::Result<Event>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if let Some(stream) = this.stream.as_mut().as_pin_mut() {
            match stream.poll_next(cx) {
                Poll::Ready(Some(Err(err))) => {
                    this.stream.set(None);
                    Poll::Ready(Some(Ok(Event::default().event("error").json_data(
                        json!({
                            "message": err.to_string(),
                        }),
                    )?)))
                }
                Poll::Ready(Some(Ok(e))) => Poll::Ready(Some(Ok(e))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(None)
        }
    }
}

pub fn join_with<T, E>(
    stream: impl Stream<Item = Result<T, E>>,
    task: impl Future<Output = Result<(), E>>,
) -> impl Stream<Item = Result<T, E>> {
    StreamJoin {
        stream: Some(stream),
        task: Some(task),
    }
}

#[pin_project::pin_project]
pub struct StreamJoin<S: Stream<Item = Result<T, E>>, F: Future<Output = Result<(), E>>, T, E> {
    #[pin]
    stream: Option<S>,
    #[pin]
    task: Option<F>,
}

impl<S: Stream<Item = Result<T, E>>, F: Future<Output = Result<(), E>>, T, E> Stream
    for StreamJoin<S, F, T, E>
{
    type Item = Result<T, E>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if let Some(stream) = this.stream.as_mut().as_pin_mut() {
            match stream.poll_next(cx) {
                Poll::Ready(None) => this.stream.set(None),
                v @ Poll::Ready(Some(_)) => return v,
                Poll::Pending => {}
            }
        }

        if let Some(task) = this.task.as_mut().as_pin_mut() {
            match task.poll(cx) {
                Poll::Ready(Ok(())) => this.task.set(None),
                Poll::Ready(Err(err)) => {
                    this.stream.set(None);
                    this.task.set(None);
                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if this.stream.is_none() && this.task.is_none() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

pub fn stream_until<T, E>(
    stream: impl Stream<Item = Result<T, E>>,
    deadline: std::time::Instant,
    timeout_response: Result<T, E>,
) -> impl Stream<Item = Result<T, E>> {
    StreamTimeout {
        stream: Some(stream),
        sleep: tokio::time::sleep_until(deadline.into()),
        timeout_response: Some(timeout_response),
    }
}

#[pin_project::pin_project]
pub struct StreamTimeout<S: Stream<Item = Result<T, E>>, T, E> {
    #[pin]
    stream: Option<S>,
    #[pin]
    sleep: tokio::time::Sleep,
    timeout_response: Option<Result<T, E>>,
}

impl<S: Stream<Item = Result<T, E>>, T, E> Stream for StreamTimeout<S, T, E> {
    type Item = Result<T, E>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if let Some(stream) = this.stream.as_mut().as_pin_mut() {
            match this.sleep.poll(cx) {
                Poll::Ready(()) => {
                    this.stream.set(None);
                    return Poll::Ready(Some(this.timeout_response.take().unwrap()));
                }
                Poll::Pending => {}
            }

            match stream.poll_next(cx) {
                Poll::Ready(None) => {
                    this.stream.set(None);
                    Poll::Ready(None)
                }
                v => v,
            }
        } else {
            Poll::Ready(None)
        }
    }
}
