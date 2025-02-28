use std::{borrow::Cow, future::Future, pin::Pin};

use anyhow::anyhow;
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream};
use promptkit_request::{
    RequestContext, RequestOptions, TraceRequest, WebsocketMessage, request_span,
};
use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tokio::{sync::mpsc::error::SendError, task::JoinHandle};
use tracing::{Instrument, field::Empty};

use promptkit_executor::{
    Env,
    env::{RpcConnect, RpcPayload},
};
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use url::Url;

#[derive(Clone)]
pub struct VmEnv {
    pub client: promptkit_request::Client,
}

impl VmEnv {
    pub fn update(&self) -> Cow<'_, Self> {
        Cow::Borrowed(self)
    }
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
    type Error = anyhow::Error;

    fn hash(&self, _update: impl FnMut(&[u8])) {}

    fn send_request_http<B>(
        &self,
        request: http::Request<B>,
    ) -> impl Future<
        Output = anyhow::Result<
            http::Response<
                Pin<
                    Box<
                        dyn futures_core::Stream<Item = anyhow::Result<http_body::Frame<Bytes>>>
                            + Send
                            + Sync
                            + 'static,
                    >,
                >,
            >,
        >,
    > + Send
    + 'static
    where
        B: http_body::Body + Send + Sync + 'static,
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
        async move {
            let resp = http.await.map_err(anyhow::Error::from_boxed)?;
            Ok(resp.map(
                |b| -> Pin<
                    Box<
                        dyn futures_core::Stream<Item = anyhow::Result<http_body::Frame<Bytes>>>
                            + Send
                            + Sync
                            + 'static,
                    >,
                > { Box::pin(b.map_err(anyhow::Error::from_boxed)) },
            ))
        }
    }

    fn connect_rpc(
        &self,
        connect: RpcConnect,
        req: tokio::sync::mpsc::Receiver<RpcPayload>,
        resp: tokio::sync::mpsc::Sender<anyhow::Result<RpcPayload>>,
    ) -> impl Future<Output = Result<JoinHandle<anyhow::Result<()>>, Self::Error>> + Send + 'static
    {
        let url = Url::parse(&connect.url).unwrap();
        let client = self.client.clone();
        async move {
            let timeout = connect.timeout;
            if url.scheme() == "ws" || url.scheme() == "wss" {
                let fut = websocket(client, connect, req, resp);
                if let Some(d) = timeout {
                    tokio::time::timeout(d, fut)
                        .await
                        .unwrap_or_else(|_| Err(anyhow!("timeout")))
                } else {
                    fut.await
                }
            } else if url.scheme() == "grpc" || url.scheme() == "grpcs" {
                grpc(&client, connect, req, resp)
            } else {
                Err(anyhow!("unsupported protocol"))
            }
        }
        .in_current_span()
    }
}

async fn websocket(
    client: promptkit_request::Client,
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
        .map_err(anyhow::Error::from_boxed)?
        .into_body();

    Ok(tokio::spawn(async move {
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
    }))
}

fn grpc(
    client: &promptkit_request::Client,
    mut connect: RpcConnect,
    req: tokio::sync::mpsc::Receiver<RpcPayload>,
    resp: tokio::sync::mpsc::Sender<Result<RpcPayload, anyhow::Error>>,
) -> anyhow::Result<JoinHandle<anyhow::Result<()>>> {
    let mut r = http::Request::builder().uri("http".to_owned() + &connect.url[4..]);
    for (k, v) in connect.metadata.take().unwrap_or_default() {
        r = r.header(k, v);
    }
    let req = tokio_stream::wrappers::ReceiverStream::new(req);

    let ctx = Context {
        make_span: Some(|r: &TraceRequest| {
            request_span!(
                r,
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::INFO,
                "grpc.request",
            )
        }),
    };
    let rx = client.grpc(
        r.body(req.map(|v| Bytes::from(v.data)))?,
        RequestOptions::new(ctx),
    );
    Ok(tokio::task::spawn(async move {
        let rx = rx.await.map_err(anyhow::Error::from_boxed)?.into_body();
        rx.map(|msg| match msg {
            Ok(v) => Ok(Some(RpcPayload {
                content_type: None,
                data: v.to_vec(),
            })),
            Err(v) => Err(anyhow!("{:?}", v)),
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
    }))
}
