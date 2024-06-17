use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use tracing::Instrument;
use wasmtime::component::Linker;
use wasmtime_wasi::{Pollable, ResourceTable};

use self::bindings::client::{
    Host, HostFutureResponse, HostRequest, HostResponse, HttpError, InputStream, Method,
    RequestError,
};
use self::types::{FutureResponse, Request, Response, ResponseBody};

wasmtime::component::bindgen!({
    path: "../../apis/wit",
    interfaces: "import promptkit:http/client;",
    async: true,
    trappable_imports: true,
    with: {
        "wasi": wasmtime_wasi::bindings,

        "promptkit:http/client/request": types::Request,
        "promptkit:http/client/response": types::Response,
        "promptkit:http/client/future-response": types::FutureResponse,
    }
});
pub use promptkit::http as bindings;

mod types {
    use std::{pin::Pin, time::Duration};

    use bytes::Bytes;
    use futures_util::StreamExt;
    use tracing::Instrument;
    use wasmtime_wasi::{
        runtime::AbortOnDropJoinHandle, HostInputStream, StreamError, StreamResult, Subscribe,
    };

    use super::bindings::client::HttpError;

    pub struct Request {
        pub(crate) method: reqwest::Method,
        pub(crate) url: Option<reqwest::Url>,
        pub(crate) headers: reqwest::header::HeaderMap,
        pub(crate) body: Option<Vec<u8>>,
        pub(crate) timeout: Option<Duration>,
    }

    pub enum FutureResponse {
        Pending(AbortOnDropJoinHandle<Result<Response, HttpError>>),
        Ready(Result<Response, HttpError>),
        Consumed,
    }

    #[async_trait::async_trait]
    impl Subscribe for FutureResponse {
        async fn ready(&mut self) -> () {
            if let Self::Pending(handle) = self {
                *self = Self::Ready(handle.await);
            }
        }
    }

    pub struct Response {
        pub(crate) status: u16,
        pub(crate) headers: Vec<(String, String)>,
        pub(crate) body: Option<(reqwest::Response, tracing::Span)>,
    }

    pub(crate) struct ResponseBody {
        pub(crate) stream:
            Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
        pub(crate) buffer: Option<StreamResult<(bytes::Bytes, usize)>>,
        pub(crate) span: tracing::Span,
        pub(crate) response_size: usize,
    }

    #[async_trait::async_trait]
    impl Subscribe for ResponseBody {
        async fn ready(&mut self) -> () {
            if self.buffer.is_some() {
                return;
            }
            self.buffer = Some(
                match self.stream.next().instrument(self.span.clone()).await {
                    Some(Ok(b)) => {
                        self.response_size += b.len();
                        Ok((b, 0))
                    }
                    Some(Err(e)) => {
                        self.span.record("otel.status_code", "ERROR");
                        self.span = tracing::Span::none();
                        Err(wasmtime_wasi::StreamError::LastOperationFailed(
                            anyhow::anyhow!("{}", e.to_string()),
                        ))
                    }
                    None => {
                        self.span.record("otel.status_code", "OK");
                        self.span
                            .record("http.response.body_size", self.response_size);
                        self.span = tracing::Span::none();
                        Err(wasmtime_wasi::StreamError::Closed)
                    }
                },
            );
        }
    }

    impl HostInputStream for ResponseBody {
        fn read(&mut self, size: usize) -> StreamResult<Bytes> {
            match self.buffer.as_mut() {
                Some(Ok((b, offset))) => {
                    if size + *offset < b.len() {
                        let out = b.slice(*offset..(*offset + size));
                        *offset += size;
                        return Ok(out);
                    }
                }
                Some(Err(StreamError::Closed)) => return Err(StreamError::Closed),
                None => return Ok(Bytes::new()),
                _ => {}
            }

            match self.buffer.take() {
                Some(Ok((b, offset))) => Ok(b.slice(offset..)),
                Some(Err(e)) => Err(e),
                None => Ok(Bytes::new()),
            }
        }
    }
}

pub trait HttpView: Send {
    fn table(&mut self) -> &mut ResourceTable;

    fn send_request(
        &mut self,
        req: reqwest::Request,
    ) -> Pin<
        Box<dyn std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send + 'static>,
    >;
}

pub fn add_to_linker<T: HttpView>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    fn type_annotate<T, F>(val: F) -> F
    where
        F: Fn(&mut T) -> &mut dyn HttpView,
    {
        val
    }
    let closure = type_annotate::<T, _>(|t| t);
    bindings::client::add_to_linker_get_host(linker, closure)
}

#[async_trait::async_trait]
impl Host for dyn HttpView + '_ {
    async fn fetch(
        &mut self,
        request: wasmtime::component::Resource<Request>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<FutureResponse>, RequestError>> {
        let req = self.table().delete(request)?;
        if let Some(url) = req.url {
            let span = tracing::span!(
                target: "promptkit::http",
                tracing::Level::INFO,
                "http::fetch",
                promptkit.user = true,
                otel.status_code = tracing::field::Empty,
                http.response.body_size = tracing::field::Empty,
            );

            let mut run = reqwest::Request::new(req.method, url);
            let _ = std::mem::replace(run.headers_mut(), req.headers);
            let _ = std::mem::replace(run.timeout_mut(), req.timeout);
            let _ = std::mem::replace(run.body_mut(), req.body.map(reqwest::Body::from));
            let resp = {
                let _guard = span.enter();
                self.send_request(run)
            };

            Ok(Ok(self.table().push(FutureResponse::Pending(
                wasmtime_wasi::runtime::spawn(async move {
                    let resp = resp.instrument(span.clone()).await;
                    let resp = match resp {
                        Ok(resp) => resp,
                        Err(e) => return Err(HttpError::Unknown(e.to_string())),
                    };

                    Ok(Response {
                        status: resp.status().as_u16(),
                        headers: resp
                            .headers()
                            .iter()
                            .map(|(k, v)| {
                                (
                                    k.as_str().to_lowercase().to_string(),
                                    v.to_str().unwrap().to_string(),
                                )
                            })
                            .collect(),
                        body: Some((resp, span)),
                    })
                }),
            ))?))
        } else {
            return Ok(Err(RequestError::InvalidUrl));
        }
    }
}

#[async_trait::async_trait]
impl HostRequest for dyn HttpView + '_ {
    async fn new(
        &mut self,
        method: Method,
    ) -> wasmtime::Result<wasmtime::component::Resource<Request>> {
        Ok(self.table().push(Request {
            method: match method {
                Method::Get => reqwest::Method::GET,
                Method::Head => reqwest::Method::HEAD,
                Method::Post => reqwest::Method::POST,
                Method::Put => reqwest::Method::PUT,
                Method::Delete => reqwest::Method::DELETE,
                Method::Connect => reqwest::Method::CONNECT,
                Method::Options => reqwest::Method::OPTIONS,
                Method::Trace => reqwest::Method::TRACE,
                Method::Patch => reqwest::Method::PATCH,
            },
            url: None,
            headers: reqwest::header::HeaderMap::new(),
            timeout: None,
            body: None,
        })?)
    }

    async fn set_url(
        &mut self,
        request: wasmtime::component::Resource<Request>,
        url: String,
    ) -> wasmtime::Result<Result<(), RequestError>> {
        match reqwest::Url::parse(&url) {
            Ok(u) => self.table().get_mut(&request)?.url.insert(u),
            Err(_) => return Ok(Err(RequestError::InvalidUrl)),
        };
        Ok(Ok(()))
    }

    async fn set_header(
        &mut self,
        request: wasmtime::component::Resource<Request>,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), RequestError>> {
        match (
            reqwest::header::HeaderName::from_str(&key),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            (Ok(key), Ok(value)) => self.table().get_mut(&request)?.headers.append(key, value),
            _ => return Ok(Err(RequestError::InvalidHeader)),
        };
        Ok(Ok(()))
    }

    async fn set_timeout(
        &mut self,
        request: wasmtime::component::Resource<Request>,
        timeout_ns: u64,
    ) -> wasmtime::Result<()> {
        self.table().get_mut(&request)?.timeout = Some(Duration::from_nanos(timeout_ns));
        Ok(())
    }

    async fn write_body(
        &mut self,
        request: wasmtime::component::Resource<Request>,
        body: Vec<u8>,
    ) -> wasmtime::Result<Result<(), RequestError>> {
        let b = &mut self.table().get_mut(&request)?.body;
        match b {
            Some(b) => b.extend_from_slice(&body),
            None => {
                *b = Some(body);
            }
        };
        Ok(Ok(()))
    }

    fn drop(&mut self, request: wasmtime::component::Resource<Request>) -> wasmtime::Result<()> {
        self.table().delete(request)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostFutureResponse for dyn HttpView + '_ {
    async fn subscribe(
        &mut self,
        id: wasmtime::component::Resource<FutureResponse>,
    ) -> wasmtime::Result<wasmtime::component::Resource<Pollable>> {
        wasmtime_wasi::subscribe(self.table(), id)
    }

    async fn get(
        &mut self,
        id: wasmtime::component::Resource<FutureResponse>,
    ) -> wasmtime::Result<
        Option<Result<Result<wasmtime::component::Resource<Response>, HttpError>, ()>>,
    > {
        let resp = self.table().get_mut(&id)?;

        match resp {
            FutureResponse::Pending(_) => return Ok(None),
            FutureResponse::Consumed => return Ok(Some(Err(()))),
            FutureResponse::Ready(_) => {}
        }

        match std::mem::replace(resp, FutureResponse::Consumed) {
            FutureResponse::Ready(Ok(resp)) => Ok(Some(Ok(Ok(self.table().push(resp)?)))),
            FutureResponse::Ready(Err(http_err)) => Ok(Some(Ok(Err(http_err)))),
            _ => unreachable!(),
        }
    }

    fn drop(&mut self, id: wasmtime::component::Resource<FutureResponse>) -> wasmtime::Result<()> {
        let _ = self.table().delete(id)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostResponse for dyn HttpView + '_ {
    async fn status(
        &mut self,
        id: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<u16> {
        Ok(self.table().get(&id)?.status)
    }

    async fn headers(
        &mut self,
        id: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<Vec<(String, String)>> {
        Ok(self.table().get(&id)?.headers.clone())
    }

    async fn body(
        &mut self,
        id: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<InputStream>, ()>> {
        if let Some((body, span)) = self.table().get_mut(&id)?.body.take() {
            let stream = body.bytes_stream();
            let read = ResponseBody {
                stream: Box::pin(stream),
                buffer: None,
                span,
                response_size: 0,
            };
            Ok(Ok(self.table().push(InputStream::Host(Box::new(read)))?))
        } else {
            Ok(Err(()))
        }
    }

    fn drop(&mut self, id: wasmtime::component::Resource<Response>) -> wasmtime::Result<()> {
        self.table().delete(id)?;
        Ok(())
    }
}
