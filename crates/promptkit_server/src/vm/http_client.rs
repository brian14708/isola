use std::{str::FromStr, time::Duration};

use eventsource_stream::{Event, EventStreamError, Eventsource};
use tokio_stream::{Stream, StreamExt};
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::Table;

use super::bindgen::http_client;

pub(crate) trait HttpClientCtx: Send {
    fn client(&self) -> &reqwest::Client;
    fn table(&self) -> &Table;
    fn table_mut(&mut self) -> &mut Table;
}

pub struct ResponseSseBody(
    Box<dyn Stream<Item = Result<Event, EventStreamError<reqwest::Error>>> + Send + Sync + Unpin>,
);

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
        Ok(match body.0.next().await {
            Some(Ok(d)) => Some(Ok((d.id, d.event, d.data))),
            Some(Err(err)) => Some(Err(http_client::Error::Unknown(err.to_string()))),
            None => None,
        })
    }

    fn drop(&mut self, resource: Resource<http_client::ResponseSseBody>) -> wasmtime::Result<()> {
        self.table_mut().delete(resource)?;
        Ok(())
    }
}

pub struct Request(reqwest::Request);

#[async_trait::async_trait]
impl<I> http_client::Host for I
where
    I: HttpClientCtx,
{
    async fn fetch(
        &mut self,
        request: wasmtime::component::Resource<Request>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<Response>, http_client::Error>> {
        let request = self.table_mut().delete(request)?.0;
        match self.client().execute(request).await {
            Ok(response) => Ok(Ok(self.table_mut().push(Response(response))?)),
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
        Ok(self.table_mut().push(Request(reqwest::Request::new(
            match method {
                http_client::Method::Get => reqwest::Method::GET,
                http_client::Method::Post => reqwest::Method::POST,
            },
            reqwest::Url::parse(&url)?,
        )))?)
    }

    async fn set_body(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        body: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let request = self.table_mut().get_mut(&resource)?;
        *request.0.body_mut() = Some(reqwest::Body::from(body));
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
        request.0.headers_mut().insert(key, value);
        Ok(Ok(()))
    }

    async fn set_timeout(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        timeout_ms: u64,
    ) -> wasmtime::Result<()> {
        let request = self.table_mut().get_mut(&resource)?;
        *request.0.timeout_mut() = Some(Duration::from_millis(timeout_ms));
        Ok(())
    }

    fn drop(&mut self, req: wasmtime::component::Resource<Request>) -> wasmtime::Result<()> {
        self.table_mut().delete(req)?;
        Ok(())
    }
}

pub struct Response(reqwest::Response);

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
            .0
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
        Ok(response.0.status().as_u16())
    }

    async fn body(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<Result<Vec<u8>, http_client::Error>> {
        let response = self.table_mut().delete(resource)?;
        Ok(match response.0.bytes().await {
            Ok(data) => Ok(data.into()),
            Err(err) => Err(http_client::Error::Unknown(err.to_string())),
        })
    }

    async fn body_sse(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
    ) -> wasmtime::Result<wasmtime::component::Resource<ResponseSseBody>> {
        let response = self.table_mut().delete(resource)?;
        Ok(self.table_mut().push(ResponseSseBody(Box::new(
            response.0.bytes_stream().eventsource(),
        )))?)
    }

    fn drop(&mut self, rep: wasmtime::component::Resource<Response>) -> wasmtime::Result<()> {
        self.table_mut().delete(rep)?;
        Ok(())
    }
}
