use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::mpsc::error::TrySendError;
use tracing::Instrument;
use tungstenite::{
    Utf8Bytes,
    protocol::{CloseFrame, Message, frame::coding::CloseCode},
};
use wasmtime::component::Resource;
use wasmtime_wasi::{
    p2::{DynPollable, Pollable, bindings::clocks::monotonic_clock::Duration},
    runtime::AbortOnDropJoinHandle,
};

use super::{
    HostImpl, HostView,
    promptkit::script::outgoing_websocket::{
        ErrorCode, Headers, Host, HostConnectRequest, HostFutureWebsocket, HostReadStream,
        HostWebsocketConnection, HostWebsocketMessage, HostWriteStream, MessageType,
    },
};
use crate::env::EnvHttp;

struct WebsocketConnect {
    pub url: String,
    pub headers: Option<Vec<(String, String)>>,
    pub timeout: Option<std::time::Duration>,
}

pub enum WebsocketMessage {
    Text(Utf8Bytes),
    Binary(Bytes),
}

impl From<WebsocketMessage> for Message {
    fn from(msg: WebsocketMessage) -> Self {
        match msg {
            WebsocketMessage::Text(data) => Message::Text(data),
            WebsocketMessage::Binary(data) => Message::Binary(data),
        }
    }
}

impl<T: HostView> HostWebsocketMessage for HostImpl<T> {
    async fn new(
        &mut self,
        message_type: MessageType,
        data: Vec<u8>,
    ) -> wasmtime::Result<Resource<WebsocketMessage>> {
        let msg = match message_type {
            MessageType::Text => WebsocketMessage::Text(
                data.try_into()
                    .map_err(|_| anyhow::anyhow!("invalid websocket text message"))?,
            ),
            MessageType::Binary => WebsocketMessage::Binary(data.into()),
        };
        Ok(self.0.table().push(msg)?)
    }

    async fn message_type(
        &mut self,
        self_: Resource<WebsocketMessage>,
    ) -> wasmtime::Result<MessageType> {
        let msg = self.0.table().get(&self_)?;
        let msg_type = match &msg {
            WebsocketMessage::Text(_) => MessageType::Text,
            WebsocketMessage::Binary(_) => MessageType::Binary,
        };
        Ok(msg_type)
    }

    async fn read(&mut self, self_: Resource<WebsocketMessage>) -> wasmtime::Result<Vec<u8>> {
        let mut msg = self.0.table().get_mut(&self_)?;
        match &mut msg {
            WebsocketMessage::Text(data) => {
                match Into::<Bytes>::into(std::mem::take(data)).try_into_mut() {
                    Ok(bytes) => Ok(bytes.into()),
                    Err(bytes) => Ok(bytes.to_vec()),
                }
            }
            WebsocketMessage::Binary(data) => match std::mem::take(data).try_into_mut() {
                Ok(bytes) => Ok(bytes.into()),
                Err(bytes) => Ok(bytes.to_vec()),
            },
        }
    }

    async fn drop(&mut self, rep: Resource<WebsocketMessage>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

impl<T: HostView> Host for HostImpl<T> {
    async fn connect(
        &mut self,
        connect: Resource<ConnectRequest>,
    ) -> wasmtime::Result<Resource<FutureWebsocket>> {
        let conn = self.0.table().delete(connect)?;
        let env = self.0.env()?;

        let s = wasmtime_wasi::runtime::spawn({
            async move {
                // Create streams for bidirectional communication
                let (write_tx, write_rx) = tokio::sync::mpsc::channel::<Message>(16);

                // Convert WebsocketConnect to HTTP request
                let mut request = http::Request::builder()
                    .uri(&conn.0.url)
                    .body(tokio_stream::wrappers::ReceiverStream::new(write_rx))
                    .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))?;

                // Add headers if provided
                if let Some(headers) = &conn.0.headers {
                    for (key, value) in headers {
                        let header_name = key.parse::<http::HeaderName>().map_err(|e| {
                            ErrorCode::ProtocolError(Some(format!(
                                "Invalid header name '{key}': {e}"
                            )))
                        })?;
                        let header_value = value.parse::<http::HeaderValue>().map_err(|e| {
                            ErrorCode::ProtocolError(Some(format!(
                                "Invalid header value for '{key}': {e}"
                            )))
                        })?;
                        request.headers_mut().insert(header_name, header_value);
                    }
                }

                // Connect via WebSocket
                let mut response = if let Some(timeout) = conn.0.timeout {
                    tokio::time::timeout(timeout, env.connect_websocket(request))
                        .await
                        .map_err(|_| ErrorCode::Timeout)?
                } else {
                    env.connect_websocket(request).await
                }
                .map_err(|e| ErrorCode::ConnectionFailed(Some(e.to_string())))?;

                // Take ownership of the header map
                let response_headers = std::mem::take(response.headers_mut());

                // Use the response stream directly
                let response_stream = response.into_body();
                let mapped_stream =
                    response_stream.map(|result| result.map_err(|e| anyhow::anyhow!("{}", e)));
                let boxed_stream: Pin<
                    Box<dyn Stream<Item = anyhow::Result<Message>> + Send + Sync>,
                > = Box::pin(mapped_stream);

                let connection = WebsocketConnection {
                    streams: Some((
                        WriteStream::Owned(write_tx),
                        ReadStream {
                            stream: boxed_stream,
                            peek: None,
                        },
                    )),
                    headers: response_headers,
                };
                Ok(connection)
            }
            .in_current_span()
        });
        Ok(self.0.table().push(FutureWebsocket::Pending(s))?)
    }
}

pub struct ConnectRequest(WebsocketConnect);

impl<T: HostView> HostConnectRequest for HostImpl<T> {
    async fn new(
        &mut self,
        url: String,
        headers: Option<Headers>,
    ) -> wasmtime::Result<Resource<ConnectRequest>> {
        Ok(self.0.table().push(ConnectRequest(WebsocketConnect {
            url,
            headers,
            timeout: None,
        }))?)
    }

    async fn set_connect_timeout(
        &mut self,
        self_: Resource<ConnectRequest>,
        duration: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let c = self.0.table().get_mut(&self_)?;
        c.0.timeout = duration.map(std::time::Duration::from_nanos);
        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<ConnectRequest>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

pub struct ReadStream {
    stream: Pin<Box<dyn Stream<Item = anyhow::Result<Message>> + Send + Sync>>,
    peek: Option<Result<WebsocketMessage, ErrorCode>>,
}

#[async_trait::async_trait]
impl Pollable for ReadStream {
    async fn ready(&mut self) {
        if self.peek.is_none() {
            self.peek = Some(self.do_peek().await);
        }
    }
}

impl ReadStream {
    async fn do_peek(&mut self) -> Result<WebsocketMessage, ErrorCode> {
        loop {
            return match self.stream.next().await {
                Some(Ok(msg)) => match msg {
                    Message::Text(t) => Ok(WebsocketMessage::Text(t)),
                    Message::Binary(b) => Ok(WebsocketMessage::Binary(b)),
                    Message::Close(None) => {
                        Err(ErrorCode::Closed((CloseCode::Status.into(), String::new())))
                    }
                    Message::Close(Some(CloseFrame { code, reason })) => {
                        Err(ErrorCode::Closed((code.into(), reason.as_str().to_owned())))
                    }
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                },
                Some(Err(e)) => Err(ErrorCode::InternalError(Some(e.to_string()))),
                None => Err(ErrorCode::Closed((
                    CloseCode::Abnormal.into(),
                    String::new(),
                ))),
            };
        }
    }
}

impl<T: HostView> HostReadStream for HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<ReadStream>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn read(
        &mut self,
        self_: Resource<ReadStream>,
    ) -> wasmtime::Result<Option<Result<Resource<WebsocketMessage>, ErrorCode>>> {
        let stream = self.0.table().get_mut(&self_)?;
        Ok(match stream.peek.take() {
            Some(Ok(msg)) => Some(Ok(self.0.table().push(msg)?)),
            Some(Err(e)) => Some(Err(e)),
            None => match stream.do_peek().now_or_never() {
                Some(Ok(msg)) => {
                    let msg = self.0.table().push(msg)?;
                    Some(Ok(msg))
                }
                Some(Err(e)) => Some(Err(e)),
                None => None,
            },
        })
    }

    async fn drop(&mut self, rep: Resource<ReadStream>) -> wasmtime::Result<()> {
        let mut stream = self.0.table().delete(rep)?;
        // drain the stream
        while let Some(Some(_)) = stream.stream.next().now_or_never() {}
        Ok(())
    }
}

pub enum WriteStream {
    Closed,
    Owned(tokio::sync::mpsc::Sender<Message>),
    Permit(tokio::sync::mpsc::OwnedPermit<Message>),
}

impl WriteStream {
    fn map<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(WriteStream) -> (WriteStream, T),
    {
        let tmp = std::mem::replace(self, WriteStream::Closed);
        let (new, ret) = f(tmp);
        *self = new;
        ret
    }
}

#[async_trait::async_trait]
impl Pollable for WriteStream {
    async fn ready(&mut self) {
        match self {
            WriteStream::Closed | WriteStream::Permit(_) => {}
            WriteStream::Owned(sender) => {
                *self = match sender.clone().reserve_owned().await {
                    Ok(permit) => WriteStream::Permit(permit),
                    Err(_) => WriteStream::Closed,
                };
            }
        }
    }
}

impl<T: HostView> HostWriteStream for HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<WriteStream>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn check_write(
        &mut self,
        self_: Resource<WriteStream>,
        _message: Resource<WebsocketMessage>,
    ) -> wasmtime::Result<Result<bool, ErrorCode>> {
        let stream = self.0.table().get_mut(&self_)?;
        Ok(stream.map(|inner| match inner {
            WriteStream::Closed => (
                inner,
                Err(ErrorCode::ConnectionFailed(Some(
                    "Connection closed".to_string(),
                ))),
            ),
            WriteStream::Owned(sender) => match sender.try_reserve_owned() {
                Ok(permit) => (WriteStream::Permit(permit), Ok(true)),
                Err(TrySendError::Full(sender)) => (WriteStream::Owned(sender), Ok(false)),
                Err(TrySendError::Closed(_)) => (
                    WriteStream::Closed,
                    Err(ErrorCode::ConnectionFailed(Some(
                        "Connection closed".to_string(),
                    ))),
                ),
            },
            WriteStream::Permit(_) => (inner, Ok(true)),
        }))
    }

    async fn write(
        &mut self,
        self_: Resource<WriteStream>,
        message: Resource<WebsocketMessage>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let msg = self.0.table().delete(message)?;
        let stream = self.0.table().get_mut(&self_)?;
        let msg = msg.into();

        Ok(stream.map(move |inner| match inner {
            WriteStream::Closed => (
                inner,
                Err(ErrorCode::ConnectionFailed(Some(
                    "Connection closed".to_string(),
                ))),
            ),
            WriteStream::Owned(ref sender) => match sender.try_send(msg) {
                Ok(()) => (inner, Ok(())),
                Err(TrySendError::Full(_)) => (
                    inner,
                    Err(ErrorCode::InternalError(Some("Buffer full".to_string()))),
                ),
                Err(TrySendError::Closed(_)) => (
                    inner,
                    Err(ErrorCode::ConnectionFailed(Some(
                        "Connection closed".to_string(),
                    ))),
                ),
            },
            WriteStream::Permit(permit) => {
                let sender = permit.send(msg);
                (WriteStream::Owned(sender), Ok(()))
            }
        }))
    }

    async fn close(
        &mut self,
        self_: Resource<WriteStream>,
        code: u16,
        reason: String,
    ) -> wasmtime::Result<Option<Result<(), ErrorCode>>> {
        let stream = self.0.table().get_mut(&self_)?;

        Ok(stream.map(move |inner| match inner {
            WriteStream::Closed => (
                inner,
                Some(Err(ErrorCode::ConnectionFailed(Some(
                    "Connection closed".to_string(),
                )))),
            ),
            WriteStream::Owned(ref sender) => match sender.try_send(close_message(code, reason)) {
                Ok(()) => (inner, Some(Ok(()))),
                Err(TrySendError::Full(_)) => (inner, None),
                Err(TrySendError::Closed(_)) => (
                    WriteStream::Closed,
                    Some(Err(ErrorCode::ConnectionFailed(Some(
                        "Connection closed".to_string(),
                    )))),
                ),
            },
            WriteStream::Permit(permit) => {
                let _sender = permit.send(close_message(code, reason));
                (WriteStream::Closed, Some(Ok(())))
            }
        }))
    }

    async fn drop(&mut self, rep: Resource<WriteStream>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

fn close_message(code: u16, reason: String) -> Message {
    Message::Close(Some(CloseFrame {
        code: code.into(),
        reason: reason.into(),
    }))
}

pub struct WebsocketConnection {
    streams: Option<(WriteStream, ReadStream)>,
    headers: http::HeaderMap,
}

impl<T: HostView> HostWebsocketConnection for HostImpl<T> {
    async fn streams(
        &mut self,
        self_: Resource<WebsocketConnection>,
    ) -> wasmtime::Result<Result<(Resource<WriteStream>, Resource<ReadStream>), ()>> {
        let conn = self.0.table().get_mut(&self_)?;
        Ok(match conn.streams.take() {
            Some((write, read)) => Ok((
                self.0.table().push_child(write, &self_)?,
                self.0.table().push_child(read, &self_)?,
            )),
            None => Err(()),
        })
    }

    async fn headers(
        &mut self,
        self_: Resource<WebsocketConnection>,
    ) -> wasmtime::Result<Vec<(String, String)>> {
        let conn = self.0.table().get(&self_)?;
        Ok(conn
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.to_string(), v.to_string()))
            })
            .collect())
    }

    async fn drop(&mut self, rep: Resource<WebsocketConnection>) -> wasmtime::Result<()> {
        let _conn = self.0.table().delete(rep)?;
        Ok(())
    }
}

pub enum FutureWebsocket {
    Pending(AbortOnDropJoinHandle<Result<WebsocketConnection, ErrorCode>>),
    Ready(Result<WebsocketConnection, ErrorCode>),
    Consumed,
}

#[async_trait::async_trait]
impl Pollable for FutureWebsocket {
    async fn ready(&mut self) {
        if let Self::Pending(handle) = self {
            *self = Self::Ready(handle.await);
        }
    }
}

impl<T: HostView> HostFutureWebsocket for HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<FutureWebsocket>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn get(
        &mut self,
        self_: Resource<FutureWebsocket>,
    ) -> wasmtime::Result<Option<Result<Result<Resource<WebsocketConnection>, ErrorCode>, ()>>>
    {
        let future = self.0.table().get_mut(&self_)?;
        match future {
            FutureWebsocket::Ready(_) => {
                match std::mem::replace(future, FutureWebsocket::Consumed) {
                    FutureWebsocket::Ready(Ok(conn)) => {
                        Ok(Some(Ok(Ok(self.0.table().push(conn)?))))
                    }
                    FutureWebsocket::Ready(Err(e)) => Ok(Some(Ok(Err(e)))),
                    FutureWebsocket::Pending(_) | FutureWebsocket::Consumed => unreachable!(),
                }
            }
            FutureWebsocket::Pending(_) => Ok(None),
            FutureWebsocket::Consumed => Ok(Some(Err(()))),
        }
    }

    async fn drop(&mut self, rep: Resource<FutureWebsocket>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}
