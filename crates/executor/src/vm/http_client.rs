use std::{str::FromStr, time::Duration};

use bytes::Bytes;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use serde_json::{json, value::to_raw_value};
use tokio_stream::{Stream, StreamExt};
use wasmtime::component::{Resource, ResourceTable};

use crate::trace::TracerContext;

use super::bindgen::http_client::{self, SseEvent};

pub trait HttpClientCtx: Send {
    fn tracer(&self) -> &TracerContext;
    fn client(&self) -> &reqwest::Client;
    fn table(&mut self) -> &mut ResourceTable;
}

pub struct ResponseSseBody {
    inner: Box<
        dyn Stream<Item = Result<Event, EventStreamError<reqwest::Error>>> + Send + Sync + Unpin,
    >,
    span_id: Option<i16>,
}

#[async_trait::async_trait]
impl<I> http_client::HostResponseSseBody for I
where
    I: HttpClientCtx,
{
    async fn read(
        &mut self,
        resource: Resource<http_client::ResponseSseBody>,
    ) -> wasmtime::Result<Option<Result<http_client::SseEvent, http_client::Error>>> {
        let body = self.table().get_mut(&resource)?;
        Ok(match body.inner.next().await {
            Some(Ok(d)) => Some(Ok(SseEvent {
                id: d.id,
                event: d.event,
                data: d.data,
            })),
            Some(Err(err)) => Some(Err(http_client::Error::Unknown(err.to_string()))),
            None => {
                // end
                if let Some(span_id) = body.span_id {
                    self.tracer()
                        .with_async(|t| t.span_end("http", span_id, None))
                        .await;
                }
                None
            }
        })
    }

    fn drop(&mut self, resource: Resource<http_client::ResponseSseBody>) -> wasmtime::Result<()> {
        self.table().delete(resource)?;
        Ok(())
    }
}

pub struct Request {
    inner: reqwest::Request,
    eager: bool,
}

#[async_trait::async_trait]
impl<I> http_client::Host for I
where
    I: HttpClientCtx + Sync,
{
    async fn fetch(
        &mut self,
        request: wasmtime::component::Resource<Request>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<Response>, http_client::Error>> {
        let request = self.table().delete(request)?;
        let inner = request.inner;

        let span_id = self
            .tracer()
            .with_async(|f| {
                f.span_begin(
                    "http",
                    None,
                    "request".into(),
                    to_raw_value(&json!({
                        "url": inner.url().to_string(),
                        "method": inner.method().to_string(),
                    }))
                    .ok(),
                )
            })
            .await;

        let exec = self.client().execute(inner).await;

        self.tracer()
            .with_async(|f| async {
                f.event(
                    "http",
                    span_id,
                    "response_start".into(),
                    to_raw_value(&json!({
                        "status": exec
                            .as_ref()
                            .map(|r| r.status().as_u16())
                            .unwrap_or_default(),
                    }))
                    .ok(),
                )
                .await;

                if let (true, Some(span_id)) = (request.eager, span_id) {
                    f.span_end("http", span_id, None).await;
                }
            })
            .await;

        match exec {
            Ok(response) => {
                let r = if request.eager {
                    let headers = response.headers().clone();
                    let status = response.status();
                    let body = match response.bytes().await {
                        Ok(body) => body,
                        Err(err) => {
                            return Ok(Err(http_client::Error::Unknown(format!(
                                "failed to read response body: {err}"
                            ))))
                        }
                    };
                    Response {
                        kind: ResponseKind::Eager {
                            headers,
                            status,
                            body,
                        },
                        span_id: None,
                    }
                } else {
                    Response {
                        kind: ResponseKind::Lazy(response),
                        span_id,
                    }
                };
                Ok(Ok(self.table().push(r)?))
            }
            Err(err) => Ok(Err(http_client::Error::Unknown(err.to_string()))),
        }
    }

    async fn fetch_all(
        &mut self,
        requests: Vec<wasmtime::component::Resource<Request>>,
    ) -> wasmtime::Result<Vec<Result<wasmtime::component::Resource<Response>, http_client::Error>>>
    {
        let requests = requests
            .into_iter()
            .map(|r| self.table().delete(r))
            .collect::<Result<Vec<_>, _>>()?;

        if requests.iter().any(|r| !r.eager) {
            return Err(anyhow::anyhow!("fetch_all only supports eager requests"));
        }

        let span_id = self
            .tracer()
            .with_async(|f| {
                f.span_begin(
                    "http",
                    None,
                    "request-multi".into(),
                    to_raw_value(
                        &requests
                            .iter()
                            .map(|r| {
                                json!({
                                    "url": r.inner.url().to_string(),
                                    "method": r.inner.method().to_string(),
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .ok(),
                )
            })
            .await;

        let client = self.client();
        let tracer = self.tracer();
        let ret = futures_util::future::join_all(requests.into_iter().enumerate().map(
            |(idx, r)| async move {
                let inner = r.inner;
                let r = client
                    .execute(inner)
                    .await
                    .map_err(|e| http_client::Error::Unknown(e.to_string()))?;
                let headers = r.headers().clone();
                let status = r.status();
                let body = r
                    .bytes()
                    .await
                    .map_err(|e| http_client::Error::Unknown(e.to_string()))?;
                if let Some(span_id) = span_id {
                    tracer
                        .with_async(|f| {
                            f.event(
                                "http",
                                Some(span_id),
                                "response-finished".into(),
                                to_raw_value(&json!({
                                    "status": status.as_u16(),
                                    "index": idx,
                                }))
                                .ok(),
                            )
                        })
                        .await;
                }
                Ok((
                    Response {
                        kind: ResponseKind::Eager {
                            headers,
                            status,
                            body,
                        },
                        span_id: None,
                    },
                    status,
                ))
            },
        ))
        .await;

        if let Some(span_id) = span_id {
            self.tracer()
                .with_async(|f| f.span_end("http", span_id, None))
                .await;
        }

        let mut result = vec![];
        for r in ret {
            match r {
                Ok((r, _)) => result.push(Ok(self.table().push(r)?)),
                Err(err) => result.push(Err(err)),
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<I> http_client::HostRequest for I
where
    I: HttpClientCtx,
{
    async fn new(
        &mut self,
        url: String,
        method: http_client::Method,
    ) -> wasmtime::Result<wasmtime::component::Resource<Request>> {
        Ok(self.table().push(Request {
            inner: reqwest::Request::new(
                match method {
                    http_client::Method::Get => reqwest::Method::GET,
                    http_client::Method::Post => reqwest::Method::POST,
                },
                reqwest::Url::parse(&url)?,
            ),
            eager: false,
        })?)
    }

    async fn set_body(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        body: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let request = self.table().get_mut(&resource)?;
        *request.inner.body_mut() = Some(reqwest::Body::from(body));
        Ok(())
    }

    async fn set_header(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), http_client::Error>> {
        let request = self.table().get_mut(&resource)?;
        let key = match reqwest::header::HeaderName::from_str(&key) {
            Ok(key) => key,
            Err(e) => return Ok(Err(http_client::Error::Unknown(e.to_string()))),
        };
        let value = match reqwest::header::HeaderValue::from_str(&value) {
            Ok(value) => value,
            Err(e) => return Ok(Err(http_client::Error::Unknown(e.to_string()))),
        };
        request.inner.headers_mut().insert(key, value);
        Ok(Ok(()))
    }

    async fn set_timeout(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        timeout_ms: u64,
    ) -> wasmtime::Result<()> {
        let request = self.table().get_mut(&resource)?;
        *request.inner.timeout_mut() = Some(Duration::from_millis(timeout_ms));
        Ok(())
    }

    async fn set_eager(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        eager: bool,
    ) -> wasmtime::Result<()> {
        let request = self.table().get_mut(&resource)?;
        request.eager = eager;
        Ok(())
    }

    fn drop(&mut self, req: wasmtime::component::Resource<Request>) -> wasmtime::Result<()> {
        self.table().delete(req)?;
        Ok(())
    }
}

pub struct Response {
    kind: ResponseKind,
    span_id: Option<i16>,
}

enum ResponseKind {
    Eager {
        headers: reqwest::header::HeaderMap,
        status: reqwest::StatusCode,
        body: Bytes,
    },
    Lazy(reqwest::Response),
}

#[async_trait::async_trait]
impl<I> http_client::HostResponse for I
where
    I: HttpClientCtx,
{
    async fn header(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
        key: String,
    ) -> wasmtime::Result<Option<String>> {
        let response = self.table().get(&resource)?;
        Ok(match &response.kind {
            ResponseKind::Eager { headers, .. } => headers,
            ResponseKind::Lazy(response) => response.headers(),
        }
        .get(&key)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string))
    }

    async fn status(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<u16> {
        let response = self.table().get(&resource)?;
        Ok(match &response.kind {
            ResponseKind::Eager { status, .. } => *status,
            ResponseKind::Lazy(response) => response.status(),
        }
        .as_u16())
    }

    async fn body(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<Result<Vec<u8>, http_client::Error>> {
        let response = self.table().delete(resource)?;

        let body = match response.kind {
            ResponseKind::Eager { body, .. } => Ok(body),
            ResponseKind::Lazy(response) => response.bytes().await,
        };

        if let Some(span_id) = response.span_id {
            self.tracer()
                .with_async(|t| {
                    t.span_end(
                        "http",
                        span_id,
                        to_raw_value(&json!({
                            "content_length": body.as_ref().map(Bytes::len).unwrap_or_default()
                        }))
                        .ok(),
                    )
                })
                .await;
        }

        Ok(match body {
            Ok(data) => Ok(data.into()),
            Err(err) => Err(http_client::Error::Unknown(err.to_string())),
        })
    }

    async fn body_sse(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<wasmtime::component::Resource<ResponseSseBody>> {
        let response = self.table().delete(resource)?;

        let body = match response.kind {
            ResponseKind::Eager { .. } => {
                return Err(anyhow::anyhow!("SSE is not supported for eager responses"))
            }
            ResponseKind::Lazy(response) => response.bytes_stream().eventsource(),
        };

        Ok(self.table().push(ResponseSseBody {
            inner: Box::new(body),
            span_id: response.span_id,
        })?)
    }

    fn drop(&mut self, rep: wasmtime::component::Resource<Response>) -> wasmtime::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}
