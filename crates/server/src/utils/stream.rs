use std::{future::Future, pin::Pin, task::Poll};

use pin_project_lite::pin_project;
use tokio_stream::Stream;

pub fn join_with_infallible<T>(
    stream: impl Stream<Item = T>,
    task: impl Future<Output = ()>,
) -> impl Stream<Item = T> {
    StreamJoinInfallible {
        stream: Some(stream),
        task: Some(task),
    }
}

pin_project! {
    pub struct StreamJoinInfallible<S: Stream<Item = T>, F: Future<Output = ()>, T> {
        #[pin]
        stream: Option<S>,
        #[pin]
        task: Option<F>,
    }
}

impl<S: Stream<Item = T>, F: Future<Output = ()>, T> Stream for StreamJoinInfallible<S, F, T> {
    type Item = T;

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
                Poll::Ready(()) => this.task.set(None),
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
