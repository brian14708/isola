use std::{task::Poll, time::Duration};

use axum::{
    http::{header::CONTENT_TYPE, HeaderValue},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
};
use pin_project_lite::pin_project;
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

pin_project! {
    struct StreamUntilError<S> {
        #[pin]
        stream: Option<S>,
    }
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
