use std::{future::Future, pin::Pin, task::Poll};

use pin_project_lite::pin_project;
use tokio_stream::Stream;

pub fn join_with<T, E>(
    stream: impl Stream<Item = Result<T, E>>,
    task: impl Future<Output = Result<(), E>>,
) -> impl Stream<Item = Result<T, E>> {
    StreamJoin {
        stream: Some(stream),
        task: Some(task),
    }
}

pin_project! {
    pub struct StreamJoin<S: Stream<Item = Result<T, E>>, F: Future<Output = Result<(), E>>, T, E> {
        #[pin]
        stream: Option<S>,
        #[pin]
        task: Option<F>,
    }
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
    span: tracing::Span,
    timeout_response: Result<T, E>,
) -> impl Stream<Item = Result<T, E>> {
    StreamTimeout {
        stream: Some(stream),
        sleep: tokio::time::sleep_until(deadline.into()),
        span,
        timeout_response: Some(timeout_response),
    }
}

pin_project! {
    pub struct StreamTimeout<S: Stream<Item = Result<T, E>>, T, E> {
        #[pin]
        stream: Option<S>,
        #[pin]
        sleep: tokio::time::Sleep,
        span: tracing::Span,
        timeout_response: Option<Result<T, E>>,
    }
}

impl<S: Stream<Item = Result<T, E>>, T, E> Stream for StreamTimeout<S, T, E> {
    type Item = Result<T, E>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let _enter = this.span.enter();

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
