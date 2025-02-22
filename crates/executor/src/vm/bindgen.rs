wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../apis/wit",
    async: true,
    trappable_imports: true,
    with: {
        "wasi:io": wasmtime_wasi::bindings::io,
        "wasi:logging": crate::wasm::logging::bindings,
        "promptkit:script/host/value-iterator": host::ValueIterator,
    },
});

pub use exports::promptkit::script::guest;

pub mod host {
    use std::pin::Pin;

    use futures_util::{FutureExt, StreamExt};
    use tokio_stream::Stream;
    use wasmtime_wasi::{Pollable, bindings::io::streams::StreamError};

    pub use super::promptkit::script::host::*;

    pub struct ValueIterator {
        pub(crate) stream: Pin<Box<dyn Stream<Item = Value> + Send>>,
        pub(crate) peek: Option<Result<Value, StreamError>>,
    }

    impl ValueIterator {
        pub fn new(stream: Pin<Box<dyn Stream<Item = Value> + Send>>) -> Self {
            Self { stream, peek: None }
        }

        pub async fn next(&mut self) -> Result<Value, StreamError> {
            match self.peek.take() {
                Some(v) => v,
                None => match self.stream.next().await {
                    Some(v) => Ok(v),
                    None => Err(StreamError::Closed),
                },
            }
        }

        pub fn try_next(&mut self) -> Option<Result<Value, StreamError>> {
            match self.peek.take() {
                Some(v) => Some(v),
                None => match self.stream.next().now_or_never() {
                    None => None,
                    Some(None) => Some(Err(StreamError::Closed)),
                    Some(Some(v)) => Some(Ok(v)),
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl Pollable for ValueIterator {
        async fn ready(&mut self) {
            if self.peek.is_none() {
                self.peek = match self.stream.next().await {
                    Some(v) => Some(Ok(v)),
                    None => Some(Err(StreamError::Closed)),
                }
            }
        }
    }
}
