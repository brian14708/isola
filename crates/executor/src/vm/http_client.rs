use std::{str::FromStr, time::Duration};

use bytes::Bytes;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use tokio_stream::{Stream, StreamExt};
use tracing::{field::Empty, span, Instrument, Span};
use wasmtime::component::Resource;

use super::{
    bindgen::http_client::{self, SseEvent},
    state::EnvCtx,
};

pub struct ResponseSseBody {
    inner: Box<
        dyn Stream<Item = Result<Event, EventStreamError<reqwest::Error>>> + Send + Sync + Unpin,
    >,
    span: Span,
}

#[async_trait::async_trait]
impl<I> http_client::HostResponseSseBody for I
where
    I: EnvCtx,
{
    async fn read(
        &mut self,
        resource: Resource<http_client::ResponseSseBody>,
    ) -> wasmtime::Result<Option<Result<http_client::SseEvent, http_client::Error>>> {
        let body = self.table().get_mut(&resource)?;
        let result = body.inner.next().instrument(body.span.clone()).await;
        Ok(match result {
            Some(Ok(d)) => Some(Ok(SseEvent {
                id: d.id,
                event: d.event,
                data: d.data,
            })),
            Some(Err(err)) => {
                body.span.record("otel.status_code", "ERROR");
                Some(Err(http_client::Error::Unknown(err.to_string())))
            }
            None => {
                body.span.record("otel.status_code", "OK");
                let _ = std::mem::replace(&mut body.span, Span::none());
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
    validate_status: bool,
}

#[async_trait::async_trait]
impl<I> http_client::Host for I
where
    I: EnvCtx + Sync,
{
    async fn fetch(
        &mut self,
        request: wasmtime::component::Resource<Request>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<Response>, http_client::Error>> {
        let request: Request = self.table().delete(request)?;
        let span = span!(
            target: "promptkit::http",
            tracing::Level::INFO,
            "http::fetch",
            promptkit.user = true,
            otel.status_code = Empty,
            http.response.body_size = Empty,
        );

        let inner = request.inner;

        let exec = self.send_request(inner).instrument(span.clone()).await;

        match exec {
            Ok(response) => {
                if request.validate_status && !response.status().is_success() {
                    span.record("otel.status_code", "ERROR");
                    return Ok(Err(http_client::Error::StatusCode(
                        response.status().as_u16(),
                    )));
                }
                let r = if request.eager {
                    let headers = response.headers().clone();
                    let status = response.status();
                    let body = match response.bytes().instrument(span.clone()).await {
                        Ok(body) => {
                            span.record("http.response.body_size", body.len() as u64);
                            span.record("otel.status_code", "OK");
                            body
                        }
                        Err(err) => {
                            span.record("otel.status_code", "ERROR");
                            return Ok(Err(http_client::Error::Unknown(format!(
                                "failed to read response body: {err}"
                            ))));
                        }
                    };
                    Response {
                        kind: ResponseKind::Eager {
                            headers,
                            status,
                            body,
                        },
                    }
                } else {
                    Response {
                        kind: ResponseKind::Lazy { response, span },
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
        ignore_error: bool,
    ) -> wasmtime::Result<Vec<Result<wasmtime::component::Resource<Response>, http_client::Error>>>
    {
        let span = span!(
            target: "promptkit::http",
            tracing::Level::INFO,
            "http::fetch_all",
            promptkit.user = true,
            otel.status_code = Empty,
        );
        let cnt = requests.len();
        let requests = requests
            .into_iter()
            .map(|r| self.table().delete(r))
            .collect::<Result<Vec<_>, _>>()?;

        if requests.iter().any(|r| !r.eager) {
            return Err(anyhow::anyhow!("fetch_all only supports eager requests"));
        }

        let client = &self;
        let tasks = requests.into_iter().map(|r| async move {
            let span = span!(
                target: "promptkit::http",
                tracing::Level::INFO,
                "http::fetch",
                promptkit.user = true,
                otel.status_code = Empty,
                http.response.body_size = Empty,
            );
            let inner = r.inner;
            let resp = client
                .send_request(inner)
                .instrument(span.clone())
                .await
                .map_err(|e| http_client::Error::Unknown(e.to_string()))?;

            if r.validate_status && !resp.status().is_success() {
                span.record("otel.status_code", "ERROR");
                return Err(http_client::Error::StatusCode(resp.status().as_u16()));
            }

            let headers = resp.headers().clone();
            let status = resp.status();
            let body = resp
                .bytes()
                .instrument(span.clone())
                .await
                .map_err(|e| http_client::Error::Unknown(e.to_string()))?;
            span.record("http.response.body_size", body.len() as u64);
            span.record("otel.status_code", "OK");
            Ok((
                Response {
                    kind: ResponseKind::Eager {
                        headers,
                        status,
                        body,
                    },
                },
                status,
            ))
        });

        if ignore_error {
            let ret = futures_util::future::join_all(tasks)
                .instrument(span.clone())
                .await;

            let mut result = vec![];
            for r in ret {
                match r {
                    Ok((r, _)) => result.push(Ok(self.table().push(r)?)),
                    Err(err) => result.push(Err(err)),
                }
            }
            span.record("otel.status_code", "OK");
            Ok(result)
        } else {
            let ret = futures_util::future::try_join_all(tasks.enumerate().map(
                |(idx, fut)| async move {
                    match fut.await {
                        Ok(response) => Ok(response),
                        Err(err) => Err((idx, err)),
                    }
                },
            ))
            .instrument(span.clone())
            .await;

            match ret {
                Ok(ret) => {
                    span.record("otel.status_code", "OK");
                    let mut result = vec![];
                    for (r, _) in ret {
                        result.push(Ok(self.table().push(r)?));
                    }
                    Ok(result)
                }
                Err((idx, err)) => {
                    span.record("otel.status_code", "ERROR");
                    let mut result = Vec::with_capacity(cnt);
                    for _ in 0..cnt {
                        result.push(Err(http_client::Error::Cancelled));
                    }
                    result[idx] = Err(err);
                    Ok(result)
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl<I> http_client::HostRequest for I
where
    I: EnvCtx,
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
            validate_status: false,
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

    async fn set_validate_status(
        &mut self,
        resource: wasmtime::component::Resource<Request>,
        validate: bool,
    ) -> wasmtime::Result<()> {
        let request = self.table().get_mut(&resource)?;
        request.validate_status = validate;
        Ok(())
    }

    fn drop(&mut self, req: wasmtime::component::Resource<Request>) -> wasmtime::Result<()> {
        self.table().delete(req)?;
        Ok(())
    }
}

pub struct Response {
    kind: ResponseKind,
}

enum ResponseKind {
    Eager {
        headers: reqwest::header::HeaderMap,
        status: reqwest::StatusCode,
        body: Bytes,
    },
    Lazy {
        response: reqwest::Response,
        span: Span,
    },
}

#[async_trait::async_trait]
impl<I> http_client::HostResponse for I
where
    I: EnvCtx,
{
    async fn header(
        &mut self,
        resource: wasmtime::component::Resource<Response>,
        key: String,
    ) -> wasmtime::Result<Option<String>> {
        let response = self.table().get(&resource)?;
        Ok(match &response.kind {
            ResponseKind::Eager { headers, .. } => headers,
            ResponseKind::Lazy { response, .. } => response.headers(),
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
            ResponseKind::Lazy { response, .. } => response.status(),
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
            ResponseKind::Lazy { response, span } => {
                let body = response.bytes().instrument(span.clone()).await;
                if body.is_ok() {
                    span.record(
                        "http.response.body_size",
                        body.as_ref().map(Bytes::len).unwrap_or_default() as u64,
                    );
                    span.record("otel.status_code", "OK");
                } else {
                    span.record("otel.status_code", "ERROR");
                }
                body
            }
        };

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

        let (body, span) = match response.kind {
            ResponseKind::Eager { .. } => {
                return Err(anyhow::anyhow!("SSE is not supported for eager responses"));
            }
            ResponseKind::Lazy { response, span } => (response.bytes_stream().eventsource(), span),
        };

        Ok(self.table().push(ResponseSseBody {
            inner: Box::new(body),
            span,
        })?)
    }

    fn drop(&mut self, rep: wasmtime::component::Resource<Response>) -> wasmtime::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::sync::Mutex;

    use futures_util::Future;
    use wasmtime::component::Resource;

    use crate::{
        vm::{http_client::http_client, state::EnvCtx},
        Env, EnvError,
    };

    struct MockEnv {
        table: Mutex<wasmtime_wasi::ResourceTable>,
    }

    impl Env for MockEnv {
        fn send_request(
            &self,
            mut request: reqwest::Request,
        ) -> impl Future<Output = Result<reqwest::Response, EnvError>> + Send {
            async move {
                match request.url().as_str() {
                    "http://example.com/echo" => Ok(http::response::Builder::new()
                        .status(http::StatusCode::OK)
                        .body::<reqwest::Body>(request.body_mut().take().unwrap_or_default())
                        .unwrap()
                        .into()),
                    "http://example.com/sleep" => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        Ok(http::response::Builder::new()
                            .status(http::StatusCode::OK)
                            .body::<reqwest::Body>(request.body_mut().take().unwrap_or_default())
                            .unwrap()
                            .into())
                    }
                    "http://example.com/error" => Ok(http::response::Builder::new()
                        .status(512)
                        .body(reqwest::Body::default())
                        .unwrap()
                        .into()),
                    _ => Err(EnvError::Unimplemented),
                }
            }
        }

        fn hash(&self, _update: impl FnMut(&[u8])) {}
    }

    impl EnvCtx for MockEnv {
        fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
            self.table.get_mut().unwrap()
        }
    }

    impl MockEnv {
        fn new() -> Self {
            Self {
                table: Mutex::new(wasmtime_wasi::ResourceTable::default()),
            }
        }

        async fn new_request(
            &mut self,
            url: &str,
            validate_status: bool,
        ) -> Resource<http_client::Request> {
            let request = http_client::HostRequest::new(
                self,
                "http://example.com".to_string() + url,
                http_client::Method::Get,
            )
            .await
            .unwrap();

            http_client::HostRequest::set_validate_status(
                self,
                Resource::new_borrow(request.rep()),
                validate_status,
            )
            .await
            .unwrap();

            http_client::HostRequest::set_eager(self, Resource::new_borrow(request.rep()), true)
                .await
                .unwrap();

            request
        }
    }

    #[tokio::test]
    async fn test_fetch() {
        let mut env = MockEnv::new();
        let request = env.new_request("/echo", false).await;

        http_client::HostRequest::set_body(
            &mut env,
            Resource::new_borrow(request.rep()),
            b"test".to_vec(),
        )
        .await
        .unwrap();

        let response = http_client::Host::fetch(&mut env, request)
            .await
            .unwrap()
            .unwrap();

        let body = http_client::HostResponse::body(&mut env, response)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(body, b"test");
    }

    #[tokio::test]
    async fn test_fetch_error() {
        let mut env = MockEnv::new();
        {
            let request = env.new_request("/error", true).await;
            http_client::HostRequest::set_validate_status(
                &mut env,
                Resource::new_borrow(request.rep()),
                true,
            )
            .await
            .unwrap();

            let response = http_client::Host::fetch(&mut env, request).await.unwrap();
            assert!(matches!(response, Err(http_client::Error::StatusCode(512))));
        }
        {
            let request = env.new_request("/error", false).await;

            let response = http_client::Host::fetch(&mut env, request)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                http_client::HostResponse::status(&mut env, Resource::new_borrow(response.rep()))
                    .await
                    .unwrap(),
                512
            );
        }
    }

    #[tokio::test]
    async fn test_fetch_all_error() {
        let mut env = MockEnv::new();
        {
            let r1 = env.new_request("/error", true).await;
            let r2 = env.new_request("/echo", true).await;

            let response = http_client::Host::fetch_all(&mut env, vec![r1, r2], true)
                .await
                .unwrap();
            assert!(matches!(
                response[0],
                Err(http_client::Error::StatusCode(512))
            ));
            // continue even if one request fails
            assert!(matches!(response[1], Ok(_)));
        }

        {
            let r1 = env.new_request("/error", true).await;
            let r2 = env.new_request("/sleep", true).await;

            let response = http_client::Host::fetch_all(&mut env, vec![r1, r2], false)
                .await
                .unwrap();
            assert!(matches!(
                response[0],
                Err(http_client::Error::StatusCode(512))
            ));
            assert!(matches!(response[1], Err(http_client::Error::Cancelled)));
        }
    }
}
