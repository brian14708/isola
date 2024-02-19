use std::{str::FromStr, time::Duration};

use eventsource_stream::{Event, EventStreamError, Eventsource};
use serde_json::value::to_raw_value;
use tokio_stream::{Stream, StreamExt};
use wasmtime::component::{Resource, ResourceTable};

use crate::trace::TracerContext;

use super::bindgen::http_client;

pub trait HttpClientCtx: Send {
    fn tracer(&self) -> &TracerContext;
    fn client(&self) -> &reqwest::Client;
    fn table(&self) -> &ResourceTable;
    fn table_mut(&mut self) -> &mut ResourceTable;
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
        let body = self.table_mut().get_mut(&resource)?;
        Ok(match body.inner.next().await {
            Some(Ok(d)) => Some(Ok((d.id, d.event, d.data))),
            Some(Err(err)) => Some(Err(http_client::Error::Unknown(err.to_string()))),
            None => {
                // end
                if let Some(span_id) = body.span_id {
                    self.tracer()
                        .with_async(|t| t.span_end("http", span_id, vec![]))
                        .await;
                }
                None
            }
        })
    }

    fn drop(&mut self, resource: Resource<http_client::ResponseSseBody>) -> wasmtime::Result<()> {
        self.table_mut().delete(resource)?;
        Ok(())
    }
}

pub struct Request {
    inner: reqwest::Request,
}

#[async_trait::async_trait]
impl<I> http_client::Host for I
where
    I: HttpClientCtx,
{
    async fn fetch(
        &mut self,
        request: wasmtime::component::Resource<Request>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<Response>, http_client::Error>> {
        let request = self.table_mut().delete(request)?.inner;

        let span_id = self
            .tracer()
            .with_async(|f| {
                f.span_begin(
                    "http",
                    None,
                    "request".into(),
                    vec![
                        (
                            "url".into(),
                            Some(to_raw_value(&request.url().to_string()).unwrap()),
                        ),
                        (
                            "method".into(),
                            Some(to_raw_value(&request.method().to_string()).unwrap()),
                        ),
                        (
                            "headers".into(),
                            Some(
                                to_raw_value(
                                    &request
                                        .headers()
                                        .iter()
                                        .map(|(k, v)| {
                                            (
                                                k.as_str().to_string(),
                                                v.to_str().unwrap().to_string(),
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                )
                                .unwrap(),
                            ),
                        ),
                    ],
                )
            })
            .await;

        let exec = self.client().execute(request).await;

        self.tracer()
            .with_async(|f| {
                f.event(
                    "http",
                    span_id,
                    "response_start".into(),
                    vec![(
                        "status".into(),
                        Some(
                            to_raw_value(
                                &exec
                                    .as_ref()
                                    .map(|r| r.status().as_u16())
                                    .unwrap_or_default(),
                            )
                            .unwrap(),
                        ),
                    )],
                )
            })
            .await;

        match exec {
            Ok(response) => Ok(Ok(self.table_mut().push(Response {
                inner: response,
                span_id,
            })?)),
            Err(err) => Ok(Err(http_client::Error::Unknown(err.to_string()))),
        }
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
        Ok(self.table_mut().push(Request {
            inner: reqwest::Request::new(
                match method {
                    http_client::Method::Get => reqwest::Method::GET,
                    http_client::Method::Post => reqwest::Method::POST,
                },
                reqwest::Url::parse(&url)?,
            ),
        })?)
    }

    async fn set_body(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        body: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let request = self.table_mut().get_mut(&resource)?;
        *request.inner.body_mut() = Some(reqwest::Body::from(body));
        Ok(())
    }

    async fn set_header(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), http_client::Error>> {
        let request = self.table_mut().get_mut(&resource)?;
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
        let request = self.table_mut().get_mut(&resource)?;
        *request.inner.timeout_mut() = Some(Duration::from_millis(timeout_ms));
        Ok(())
    }

    fn drop(&mut self, req: wasmtime::component::Resource<Request>) -> wasmtime::Result<()> {
        self.table_mut().delete(req)?;
        Ok(())
    }
}

pub struct Response {
    inner: reqwest::Response,
    span_id: Option<i16>,
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
        Ok(response
            .inner
            .headers()
            .get(key)
            .and_then(|e| e.to_str().ok())
            .map(|s| s.to_owned()))
    }

    async fn status(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<u16> {
        let response = self.table().get(&resource)?;
        Ok(response.inner.status().as_u16())
    }

    async fn body(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<Result<Vec<u8>, http_client::Error>> {
        let response = self.table_mut().delete(resource)?;
        let body = response.inner.bytes().await;
        if let Some(span_id) = response.span_id {
            self.tracer()
                .with_async(|t| {
                    t.span_end(
                        "http",
                        span_id,
                        vec![(
                            "content_length".into(),
                            Some(
                                to_raw_value(&body.as_ref().map(|b| b.len()).unwrap_or_default())
                                    .unwrap(),
                            ),
                        )],
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
        let response = self.table_mut().delete(resource)?;
        Ok(self.table_mut().push(ResponseSseBody {
            inner: Box::new(response.inner.bytes_stream().eventsource()),
            span_id: response.span_id,
        })?)
    }

    fn drop(&mut self, rep: wasmtime::component::Resource<Response>) -> wasmtime::Result<()> {
        self.table_mut().delete(rep)?;
        Ok(())
    }
}
