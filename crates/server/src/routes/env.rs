use std::sync::Arc;

use anyhow::anyhow;
use futures::{StreamExt, TryStreamExt, stream};
use promptkit_executor::{
    MpscOutputCallback,
    env::{BoxedStream, Env, EnvHttp, RpcConnect, RpcPayload},
};
use promptkit_request::{
    RequestContext, RequestOptions, TraceRequest, WebsocketMessage, request_span,
};
use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tokio::{sync::mpsc::error::SendError, task::JoinHandle};
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tracing::{Instrument, field::Empty};
use url::Url;

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

    async fn connect_rpc(
        &self,
        connect: RpcConnect,
        req: tokio::sync::mpsc::Receiver<RpcPayload>,
        resp: tokio::sync::mpsc::Sender<anyhow::Result<RpcPayload>>,
    ) -> Result<JoinHandle<anyhow::Result<()>>, Self::Error> {
        let url = Url::parse(&connect.url).unwrap();
        let timeout = connect.timeout;
        if url.scheme() == "ws" || url.scheme() == "wss" {
            let fut = websocket(&self.client, connect, req, resp);

            if let Some(d) = timeout {
                tokio::time::timeout(d, fut)
                    .await
                    .unwrap_or_else(|_| Err(anyhow!("timeout")))
            } else {
                fut.await
            }
        } else {
            Err(anyhow!("unsupported protocol"))
        }
    }
}

async fn websocket(
    client: &promptkit_request::Client,
    mut connect: RpcConnect,
    req: tokio::sync::mpsc::Receiver<RpcPayload>,
    resp: tokio::sync::mpsc::Sender<Result<RpcPayload, anyhow::Error>>,
) -> anyhow::Result<JoinHandle<anyhow::Result<()>>> {
    let mut r = http::Request::builder().uri("http".to_owned() + &connect.url[2..]);
    for (k, v) in connect.metadata.take().unwrap_or_default() {
        r = r.header(k, v);
    }
    let req = tokio_stream::wrappers::ReceiverStream::new(req)
        .map(|msg| {
            if msg.content_type.is_some_and(|t| t.starts_with("text")) {
                WebsocketMessage::Text(String::from_utf8(msg.data).unwrap().into())
            } else {
                WebsocketMessage::Binary(msg.data.into())
            }
        })
        .chain(stream::iter([WebsocketMessage::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "".into(),
        }))]));

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
    let rx = client
        .websocket(r.body(req)?, RequestOptions::new(ctx))
        .await
        .map_err(anyhow::Error::from)?
        .into_body();

    Ok(tokio::spawn(
        async move {
            tokio_stream::StreamExt::map(rx, |msg| match msg {
                Ok(WebsocketMessage::Text(t)) => Ok(Some(RpcPayload {
                    content_type: Some("text/plain".into()),
                    data: t.to_string().into_bytes(),
                })),
                Ok(WebsocketMessage::Binary(b)) => Ok(Some(RpcPayload {
                    content_type: None,
                    data: b.to_vec(),
                })),
                Ok(
                    WebsocketMessage::Close(_)
                    | WebsocketMessage::Ping(_)
                    | WebsocketMessage::Pong(_)
                    | WebsocketMessage::Frame(_),
                ) => Ok(None),
                Err(_) => Err(anyhow!("Error recv message")),
            })
            .then(|msg| {
                let resp = resp.clone();
                async move {
                    match msg {
                        Ok(Some(t)) => Ok::<_, SendError<_>>(resp.send(Ok(t)).await?),
                        Ok(None) => Ok(()),
                        Err(e) => Ok(resp.send(Err(e)).await?),
                    }
                }
            })
            .try_collect::<()>()
            .await?;
            Ok(())
        }
        .in_current_span(),
    ))
}
