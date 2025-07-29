use anyhow::anyhow;
use futures::FutureExt;
use tokio::sync::mpsc::error::{TryRecvError, TrySendError};
use tracing::Instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::{
    p2::{DynPollable, Pollable, bindings::clocks::monotonic_clock::Duration},
    runtime::AbortOnDropJoinHandle,
};

use super::{
    HostImpl, HostView,
    promptkit::script::outgoing_rpc::{
        ErrorCode, Host, HostConnectRequest, HostConnection, HostFutureConnection, HostPayload,
        HostRequestStream, HostResponseStream, Metadata, StreamError,
    },
};
use crate::env::{EnvHttp, RpcConnect, RpcPayload};

impl<T: HostView> Host for HostImpl<T> {
    async fn connect(
        &mut self,
        connect: Resource<ConnectRequest>,
    ) -> wasmtime::Result<Resource<FutureConnection>> {
        let (tx_resp, rx_resp) = tokio::sync::mpsc::channel(4);
        let request = ResponseStream {
            stream: rx_resp,
            peek: None,
        };
        let (response, rx_req) = RequestStream::new(4);
        let conn = self.0.table().delete(connect)?;
        let env = self.0.env()?;

        let s = wasmtime_wasi::runtime::spawn(
            async move {
                let fut = env.connect_rpc(conn.0, rx_req, tx_resp);
                let join = fut
                    .await
                    .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))?;
                Ok(Connection {
                    handle: join.into(),
                    streams: Some((response, request)),
                })
            }
            .in_current_span(),
        );
        Ok(self.0.table().push(FutureConnection::Pending(s))?)
    }
}

pub struct Payload(RpcPayload);

impl<T: HostView> HostPayload for HostImpl<T> {
    async fn new(&mut self, data: Vec<u8>) -> wasmtime::Result<Resource<Payload>> {
        Ok(self.0.table().push(Payload(RpcPayload {
            data,
            content_type: None,
        }))?)
    }

    async fn content_type(&mut self, self_: Resource<Payload>) -> wasmtime::Result<Option<String>> {
        let response = self.0.table().get(&self_)?;
        Ok(response.0.content_type.clone())
    }

    async fn set_content_type(
        &mut self,
        self_: Resource<Payload>,
        content_type: String,
    ) -> wasmtime::Result<()> {
        let response = self.0.table().get_mut(&self_)?;
        response.0.content_type = Some(content_type);
        Ok(())
    }

    async fn data(&mut self, self_: Resource<Payload>) -> wasmtime::Result<Vec<u8>> {
        let response = self.0.table().get(&self_)?;
        Ok(response.0.data.clone())
    }

    async fn set_data(&mut self, self_: Resource<Payload>, data: Vec<u8>) -> wasmtime::Result<()> {
        let response = self.0.table().get_mut(&self_)?;
        response.0.data = data;
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<Payload>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

pub struct ConnectRequest(RpcConnect);

impl<T: HostView> HostConnectRequest for HostImpl<T> {
    async fn new(
        &mut self,
        url: String,
        metadata: Option<Metadata>,
    ) -> wasmtime::Result<Resource<ConnectRequest>> {
        Ok(self.0.table().push(ConnectRequest(RpcConnect {
            url,
            metadata,
            timeout: None,
        }))?)
    }

    async fn drop(&mut self, rep: Resource<ConnectRequest>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }

    async fn set_connect_timeout(
        &mut self,
        self_: wasmtime::component::Resource<ConnectRequest>,
        duration: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let c = self.0.table().get_mut(&self_)?;
        c.0.timeout = duration.map(std::time::Duration::from_nanos);
        Ok(Ok(()))
    }
}

pub struct ResponseStream {
    stream: tokio::sync::mpsc::Receiver<anyhow::Result<RpcPayload>>,
    peek: Option<Result<anyhow::Result<RpcPayload>, StreamError>>,
}

#[async_trait::async_trait]
impl Pollable for ResponseStream {
    async fn ready(&mut self) {
        if self.peek.is_none() {
            self.peek = match self.stream.recv().await {
                Some(v) => Some(Ok(v)),
                None => Some(Err(StreamError::Closed)),
            };
        }
    }
}

impl<T: HostView> HostResponseStream for HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<ResponseStream>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn read(
        &mut self,
        self_: Resource<ResponseStream>,
    ) -> wasmtime::Result<Option<Result<Resource<Payload>, StreamError>>> {
        let response = self.0.table().get_mut(&self_)?;
        match response.peek.take() {
            Some(Ok(Ok(v))) => Ok(Some(Ok(self.0.table().push(Payload(v))?))),
            Some(Ok(Err(err))) => Ok(Some(Err(StreamError::LastOperationFailed(
                ErrorCode::InternalError(Some(err.to_string())),
            )))),
            Some(Err(e)) => Ok(Some(Err(e))),
            None => match response.stream.try_recv() {
                Ok(Ok(v)) => Ok(Some(Ok(self.0.table().push(Payload(v))?))),
                Ok(Err(err)) => Ok(Some(Err(StreamError::LastOperationFailed(
                    ErrorCode::InternalError(Some(err.to_string())),
                )))),
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => Ok(Some(Err(StreamError::Closed))),
            },
        }
    }

    async fn drop(&mut self, rep: Resource<ResponseStream>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

pub enum RequestStream {
    Owned(Option<tokio::sync::mpsc::Sender<RpcPayload>>),
    Permit(tokio::sync::mpsc::OwnedPermit<RpcPayload>),
}

impl RequestStream {
    pub fn new(cap: usize) -> (Self, tokio::sync::mpsc::Receiver<RpcPayload>) {
        let (tx, rx) = tokio::sync::mpsc::channel(cap);
        (RequestStream::Owned(Some(tx)), rx)
    }

    async fn map<F, T>(&mut self, f: F) -> T
    where
        F: AsyncFnOnce(RequestStream) -> (RequestStream, T),
    {
        let tmp = std::mem::replace(self, RequestStream::Owned(None));
        let (new, ret) = f(tmp).await;
        *self = new;
        ret
    }
}

#[async_trait::async_trait]
impl Pollable for RequestStream {
    async fn ready(&mut self) {
        self.map(|r| async {
            match r {
                Self::Owned(None) | Self::Permit(_) => (r, ()),
                Self::Owned(Some(p)) => (Self::Permit(p.reserve_owned().await.unwrap()), ()),
            }
        })
        .await;
    }
}

impl<T: HostView> HostRequestStream for HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<RequestStream>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn check_write(
        &mut self,
        self_: Resource<RequestStream>,
        _content: Resource<Payload>,
    ) -> wasmtime::Result<Result<bool, StreamError>> {
        let ch = self.0.table().get_mut(&self_)?;
        Ok(ch
            .map(|ch| async {
                match ch {
                    RequestStream::Owned(None) => (ch, Err(StreamError::Closed)),
                    RequestStream::Owned(Some(p)) => match p.try_reserve_owned() {
                        Ok(p) => (RequestStream::Permit(p), Ok(true)),
                        Err(TrySendError::Full(s)) => (RequestStream::Owned(Some(s)), Ok(false)),
                        Err(TrySendError::Closed(_)) => {
                            (RequestStream::Owned(None), Err(StreamError::Closed))
                        }
                    },
                    RequestStream::Permit(_) => (ch, Ok(true)),
                }
            })
            .await)
    }

    async fn write(
        &mut self,
        self_: Resource<RequestStream>,
        content: Resource<Payload>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let elem = self.0.table().delete(content)?;
        let ch = self.0.table().get_mut(&self_)?;

        ch.map(|ch| async {
            match ch {
                RequestStream::Owned(None) => (ch, Ok(Err(StreamError::Closed))),
                RequestStream::Owned(Some(ref p)) => match p.try_send(elem.0) {
                    Ok(()) => (ch, Ok(Ok(()))),
                    Err(TrySendError::Full(_)) => (ch, Err(anyhow!("full"))),
                    Err(TrySendError::Closed(_)) => {
                        (RequestStream::Owned(None), Ok(Err(StreamError::Closed)))
                    }
                },
                RequestStream::Permit(p) => {
                    (RequestStream::Owned(Some(p.send(elem.0))), Ok(Ok(())))
                }
            }
        })
        .await
    }

    async fn finish(
        &mut self,
        this: Resource<RequestStream>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        self.0.table().delete(this)?;
        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<RequestStream>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}

pub struct Connection {
    handle: AbortOnDropJoinHandle<anyhow::Result<()>>,
    streams: Option<(RequestStream, ResponseStream)>,
}

impl<T: HostView> HostConnection for HostImpl<T> {
    async fn streams(
        &mut self,
        self_: Resource<Connection>,
    ) -> wasmtime::Result<Result<(Resource<RequestStream>, Resource<ResponseStream>), ()>> {
        let conn = self.0.table().get_mut(&self_)?;
        match conn.streams.take() {
            Some((req, resp)) => Ok(Ok((
                self.0.table().push_child(req, &self_)?,
                self.0.table().push_child(resp, &self_)?,
            ))),
            None => Ok(Err(())),
        }
    }

    async fn drop(&mut self, rep: Resource<Connection>) -> wasmtime::Result<()> {
        let m = self.0.table().delete(rep)?;
        m.handle.now_or_never();
        Ok(())
    }
}

pub enum FutureConnection {
    Pending(AbortOnDropJoinHandle<Result<Connection, ErrorCode>>),
    Ready(Result<Connection, ErrorCode>),
    Consumed,
}

#[async_trait::async_trait]
impl Pollable for FutureConnection {
    async fn ready(&mut self) {
        if let Self::Pending(handle) = self {
            *self = Self::Ready(handle.await);
        }
    }
}

impl<T: HostView> HostFutureConnection for HostImpl<T> {
    async fn subscribe(
        &mut self,
        self_: Resource<FutureConnection>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::p2::subscribe(self.0.table(), self_)
    }

    async fn get(
        &mut self,
        self_: Resource<FutureConnection>,
    ) -> wasmtime::Result<Option<Result<Result<Resource<Connection>, ErrorCode>, ()>>> {
        let response = self.0.table().get_mut(&self_)?;
        match response {
            FutureConnection::Ready(_) => {
                match std::mem::replace(response, FutureConnection::Consumed) {
                    FutureConnection::Ready(Ok(r)) => Ok(Some(Ok(Ok(self.0.table().push(r)?)))),
                    FutureConnection::Ready(Err(e)) => Ok(Some(Ok(Err(e)))),
                    FutureConnection::Pending(_) | FutureConnection::Consumed => unreachable!(),
                }
            }
            FutureConnection::Pending(_) => Ok(None),
            FutureConnection::Consumed => Ok(Some(Err(()))),
        }
    }

    async fn drop(&mut self, rep: Resource<FutureConnection>) -> wasmtime::Result<()> {
        self.0.table().delete(rep)?;
        Ok(())
    }
}
