use std::fs::File;
use std::sync::Arc;

use anyhow::anyhow;
use parking_lot::Mutex;
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
use super::host_types;
use super::http_client;
use super::PythonVm;

pub struct VmRunState {
    pub(crate) output: mpsc::Sender<anyhow::Result<(String, bool)>>,
}

pub struct VmState {
    limiter: MemoryLimiter,
    client: reqwest::Client,
    wasi: Mutex<WasiCtx>,
    table: Mutex<ResourceTable>,
    pub(crate) tracer: Arc<TracerContext>,
    pub(crate) run: Option<VmRunState>,
}

impl VmState {
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;
        PythonVm::add_to_linker(&mut linker, |v: &mut Self| v)?;
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
                wasi: Mutex::new(wasi),
                table: Mutex::new(ResourceTable::new()),
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
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        self.wasi.get_mut()
    }
}

#[async_trait::async_trait]
impl bindgen::host::Host for VmState {
    async fn emit(&mut self, data: String) -> wasmtime::Result<()> {
        if let Some(run) = &self.run {
            run.output.send(Ok((data, false))).await?;
            Ok(())
        } else {
            Err(anyhow!("output channel missing"))
        }
    }
}

impl http_client::HttpClientCtx for VmState {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn tracer(&self) -> &TracerContext {
        &self.tracer
    }
}

impl host_types::HostTypesCtx for VmState {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }
}
