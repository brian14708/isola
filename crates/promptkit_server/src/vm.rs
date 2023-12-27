use anyhow::anyhow;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};
use wasmtime::{component::Resource, Engine, Store};
use wasmtime_wasi::preview2::{Table, WasiCtx, WasiCtxBuilder, WasiView};

use crate::resource::MemoryLimiter;

wasmtime::component::bindgen!({
    world: "python-vm",
    async: true,

    with: {
        "http-client/response-sse-body": ResponseBody,
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
    pub fn new(engine: &Engine, max_memory: usize) -> Store<Self> {
        let wasi = WasiCtxBuilder::new().build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            VmState {
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
    async fn fetch_sse(
        &mut self,
        (url, status, headers, body): http_client::Request,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<http_client::ResponseSse, String>> {
        let mut builder = self.client.request(
            match status {
                http_client::Method::Get => reqwest::Method::GET,
                http_client::Method::Post => reqwest::Method::POST,
            },
            &url,
        );
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        if timeout_ms > 0 {
            builder = builder.timeout(std::time::Duration::from_millis(timeout_ms as u64));
        }

        let response = builder.send().await.map_err(|e| anyhow!(e))?;
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
            .collect();

        let m = self.table_mut().push(ResponseBody {
            body: Box::new(response.bytes_stream().eventsource()),
        })?;

        Ok(Ok((status.as_u16(), headers, m)))
    }

    async fn fetch(
        &mut self,
        (url, status, headers, body): http_client::Request,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<http_client::Response, String>> {
        let mut builder = self.client.request(
            match status {
                http_client::Method::Get => reqwest::Method::GET,
                http_client::Method::Post => reqwest::Method::POST,
            },
            &url,
        );
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        if timeout_ms > 0 {
            builder = builder.timeout(std::time::Duration::from_millis(timeout_ms as u64));
        }

        let response = builder.send().await.map_err(|e| anyhow!(e))?;
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
            .collect();
        Ok(Ok((
            status.as_u16(),
            headers,
            response.bytes().await.map_err(|e| anyhow!(e))?.into(),
        )))
    }
}

pub struct ResponseBody {
    body: Box<
        dyn Stream<Item = Result<Event, EventStreamError<reqwest::Error>>> + Send + Sync + Unpin,
    >,
}

#[async_trait::async_trait]
impl http_client::HostResponseSseBody for VmState {
    async fn read(
        &mut self,
        resource: Resource<http_client::ResponseSseBody>,
    ) -> wasmtime::Result<Option<Result<http_client::SseEvent, String>>> {
        let body = self.table_mut().get_mut(&resource)?;
        match body.body.next().await {
            Some(Ok(d)) => return Ok(Some(Ok((d.id, d.event, d.data)))),
            Some(Err(err)) => {
                return Ok(Some(Err(err.to_string())));
            }
            None => {
                return Ok(None);
            }
        }
    }

    fn drop(&mut self, resource: Resource<http_client::ResponseSseBody>) -> wasmtime::Result<()> {
        self.table_mut().delete(resource)?;
        Ok(())
    }
}
