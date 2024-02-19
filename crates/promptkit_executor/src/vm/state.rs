use std::fs::File;
use std::sync::Arc;

use anyhow::anyhow;
use tokio::sync::mpsc;
use wasmtime::component::Linker;
use wasmtime::component::ResourceTable;
use wasmtime::{Engine, Store};
use wasmtime_wasi::preview2::DirPerms;
use wasmtime_wasi::preview2::FilePerms;
use wasmtime_wasi::preview2::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi::Dir;

use crate::trace::TracerContext;
use crate::{atomic_cell::AtomicCell, resource::MemoryLimiter, trace_output::TraceOutput};

use super::bindgen;
use super::http_client;

pub(crate) struct VmRunState {
    pub(crate) output: mpsc::Sender<anyhow::Result<(String, bool)>>,
}

pub struct VmState {
    limiter: MemoryLimiter,
    client: reqwest::Client,
    wasi: WasiCtx,
    table: ResourceTable,
    pub(crate) tracer: Arc<TracerContext>,
    pub(crate) run: Option<VmRunState>,
}

impl VmState {
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;
        bindgen::host::add_to_linker(&mut linker, |v: &mut Self| v)?;
        bindgen::promptkit::python::http_client::add_to_linker(&mut linker, |v: &mut Self| v)?;
        Ok(linker)
    }

    pub fn new(engine: &Engine, max_memory: usize) -> Store<Self> {
        let tracer = Arc::new(AtomicCell::empty());
        let wasi = WasiCtxBuilder::new()
            .preopened_dir(
                Dir::from_std_file(File::open("./wasm/target/wasm32-wasi/wasi-deps/usr").unwrap()),
                DirPerms::READ,
                FilePerms::READ,
                "/usr",
            )
            .stdout(TraceOutput::new(tracer.clone(), "stdout"))
            .stderr(TraceOutput::new(tracer.clone(), "stderr"))
            .build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                tracer,
                limiter,
                client: reqwest::Client::new(),
                wasi,
                table: ResourceTable::new(),
                run: None,
            },
        );
        s.limiter(|s| &mut s.limiter);
        s
    }

    pub const fn reuse(&self) -> bool {
        !self.limiter.exceed_soft()
    }
}

impl WasiView for VmState {
    fn table(&self) -> &ResourceTable {
        &self.table
    }

    fn table_mut(&mut self) -> &mut ResourceTable {
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
impl bindgen::host::Host for VmState {
    async fn emit(&mut self, data: String, end: bool) -> wasmtime::Result<()> {
        if let Some(run) = &self.run {
            run.output.send(Ok((data, end))).await?;
            Ok(())
        } else {
            Err(anyhow!("output channel missing"))
        }
    }
}

impl http_client::HttpClientCtx for VmState {
    fn table_mut(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn table(&self) -> &ResourceTable {
        &self.table
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn tracer(&self) -> &TracerContext {
        &self.tracer
    }
}
