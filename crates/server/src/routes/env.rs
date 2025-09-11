use std::sync::Arc;

use futures::{StreamExt, TryStreamExt};
use promptkit_cbor::{from_cbor, to_cbor};
use promptkit_executor::{
    MpscOutputCallback,
    env::{BoxedStream, Env, EnvHttp},
};
use promptkit_request::{
    RequestContext, RequestOptions, TraceRequest, WebsocketMessage, request_span,
};
use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tracing::field::Empty;

#[derive(Clone)]
pub struct VmEnv {
    pub client: Arc<promptkit_request::Client>,
}

pub struct Context<F>
where
    F: FnOnce(&TraceRequest) -> tracing::Span,
{
    make_span: Option<F>,
}

impl<F> RequestContext for Context<F>
where
    F: FnOnce(&TraceRequest) -> tracing::Span,
{
    fn make_span(&mut self, r: &TraceRequest) -> tracing::Span {
        if let Some(f) = self.make_span.take() {
            f(r)
        } else {
            tracing::Span::none()
        }
    }
}

impl Env for VmEnv {
    type Callback = MpscOutputCallback;
    type Error = anyhow::Error;

    async fn hostcall(&self, call_type: &str, payload: &[u8]) -> Result<Vec<u8>, Self::Error> {
        match call_type {
            "echo" => {
                // Simple echo - return the payload as-is
                Ok(payload.to_vec())
            }
            "add" => {
                #[derive(serde::Deserialize)]
                struct AddInput {
                    a: i32,
                    b: i32,
                }
                let p: AddInput = from_cbor(payload)?;
                Ok(to_cbor(&(p.a + p.b))?.to_vec())
            }
            _ => Err(anyhow::anyhow!("unknown")), // Unknown hostcall type
        }
    }
}

impl EnvHttp for VmEnv {
    type Error = anyhow::Error;

    async fn send_request_http<B>(
        &self,
        request: http::Request<B>,
    ) -> std::result::Result<
        http::Response<BoxedStream<http_body::Frame<bytes::Bytes>, Self::Error>>,
        Self::Error,
    >
    where
        B: http_body::Body + Send + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
    {
        let ctx = Context {
            make_span: Some(|r: &TraceRequest| {
                request_span!(
                    r,
                    target: TRACE_TARGET_SCRIPT,
                    tracing::Level::INFO,
                    "http.request",
                )
            }),
        };
        let http = self.client.http(request, RequestOptions::new(ctx));
        let resp = http.await.map_err(anyhow::Error::from)?;
        Ok(resp.map(|b| -> BoxedStream<_, _> { Box::pin(b.map_err(anyhow::Error::from)) }))
    }

    async fn connect_websocket<B>(
        &self,
        request: http::Request<B>,
    ) -> Result<http::Response<BoxedStream<WebsocketMessage, Self::Error>>, Self::Error>
    where
        B: futures::Stream<Item = WebsocketMessage> + Sync + Send + 'static,
    {
        let ctx = Context {
            make_span: Some(|r: &TraceRequest| {
                request_span!(
                    r,
                    target: TRACE_TARGET_SCRIPT,
                    tracing::Level::INFO,
                    "websocket.connect",
                )
            }),
        };

        let (parts, body) = request.into_parts();
        Ok(self
            .client
            .websocket(
                http::Request::from_parts(parts, body),
                RequestOptions::new(ctx),
            )
            .await
            .map_err(anyhow::Error::from)?
            .map(|b| -> BoxedStream<_, _> {
                Box::pin(b.filter_map(|msg| async {
                    match msg {
                        Ok(s) => Some(Ok(s)),
                        Err(e) => Some(Err(anyhow::Error::from(e))),
                    }
                }))
            }))
    }
}
