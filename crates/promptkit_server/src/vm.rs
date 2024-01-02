use std::{str::FromStr, time::Duration};

use anyhow::anyhow;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};
use wasmtime::component::Linker;
use wasmtime::{component::Resource, Engine, Store};
use wasmtime_wasi::preview2::{Table, WasiCtx, WasiCtxBuilder, WasiView};

use crate::resource::MemoryLimiter;
use crate::vm::promptkit::python::http_client;

wasmtime::component::bindgen!({
    world: "python-vm",
    async: true,

    with: {
        "promptkit:python/http-client/request": Request,
        "promptkit:python/http-client/response": Response,
        "promptkit:python/http-client/response-sse-body": ResponseSseBody,
    },
});

pub struct VmState {
    limiter: MemoryLimiter,
    client: reqwest::Client,
    wasi: WasiCtx,
    table: Table,
    output: Option<mpsc::Sender<anyhow::Result<(String, bool)>>>,
}

impl VmState {
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<VmState>::new(&engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;
        host::add_to_linker(&mut linker, |v: &mut VmState| v)?;
        http_client::add_to_linker(&mut linker, |v: &mut VmState| v)?;
        Ok(linker)
    }

    pub fn new(engine: &Engine, max_memory: usize) -> Store<Self> {
        let wasi = WasiCtxBuilder::new().build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                limiter,
                client: reqwest::Client::new(),
                wasi,
                table: Table::new(),
                output: None,
            },
        );
        s.limiter(|s| &mut s.limiter);
        s
    }

    pub fn reuse(&self) -> bool {
        !self.limiter.exceed_soft()
    }

    pub fn initialize(&mut self, sender: mpsc::Sender<anyhow::Result<(String, bool)>>) {
        self.output = Some(sender);
    }

    pub fn reset(&mut self) {
        self.output = None;
    }
}

impl WasiView for VmState {
    fn table(&self) -> &Table {
        &self.table
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }

    fn ctx(&self) -> &WasiCtx {
        &self.wasi
    }

    fn ctx_mut(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

#[async_trait::async_trait]
impl host::Host for VmState {
    async fn emit(&mut self, data: String, end: bool) -> wasmtime::Result<()> {
        if let Some(output) = &self.output {
            output.send(Ok((data, end))).await?;
            Ok(())
        } else {
            Err(anyhow!("output channel missing"))
        }
    }
}

pub struct Vm {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState>,
    pub(crate) python: PythonVm,
}

#[async_trait::async_trait]
impl http_client::Host for VmState {
    async fn fetch(
        &mut self,
        request: wasmtime::component::Resource<Request>,
    ) -> wasmtime::Result<Result<wasmtime::component::Resource<Response>, String>> {
        let request = self.table_mut().delete(request)?.0;
        match self.client.execute(request).await {
            Ok(response) => Ok(Ok(self.table_mut().push(Response(response))?)),
            Err(err) => Ok(Err(err.to_string())),
        }
    }
}

pub struct ResponseSseBody(
    Box<dyn Stream<Item = Result<Event, EventStreamError<reqwest::Error>>> + Send + Sync + Unpin>,
);

#[async_trait::async_trait]
impl http_client::HostResponseSseBody for VmState {
    async fn read(
        &mut self,
        resource: Resource<http_client::ResponseSseBody>,
    ) -> wasmtime::Result<Option<Result<http_client::SseEvent, String>>> {
        let body = self.table_mut().get_mut(&resource)?;
        Ok(match body.0.next().await {
            Some(Ok(d)) => Some(Ok((d.id, d.event, d.data))),
            Some(Err(err)) => Some(Err(err.to_string())),
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
impl http_client::HostRequest for VmState {
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
    ) -> wasmtime::Result<Result<(), String>> {
        let request = self.table_mut().get_mut(&resource)?;
        let key = match reqwest::header::HeaderName::from_str(&key) {
            Ok(key) => key,
            Err(e) => return Ok(Err(e.to_string())),
        };
        let value = match reqwest::header::HeaderValue::from_str(&value) {
            Ok(value) => value,
            Err(e) => return Ok(Err(e.to_string())),
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
impl http_client::HostResponse for VmState {
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
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        let response = self.table_mut().delete(resource)?;
        Ok(match response.0.bytes().await {
            Ok(data) => Ok(data.into()),
            Err(err) => Err(err.to_string()),
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
