mod bindgen;
mod http_client;

use std::fs::File;

use anyhow::anyhow;
pub use bindgen::exports::vm as exports;
pub use bindgen::PythonVm;
use tokio::sync::mpsc;
use wasmtime::component::Linker;
use wasmtime::component::ResourceTable;
use wasmtime::{Engine, Store};
use wasmtime_wasi::preview2::DirPerms;
use wasmtime_wasi::preview2::FilePerms;
use wasmtime_wasi::preview2::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi::Dir;

use crate::resource::MemoryLimiter;

pub struct VmState {
    limiter: MemoryLimiter,
    client: reqwest::Client,
    wasi: WasiCtx,
    table: ResourceTable,
    output: Option<mpsc::Sender<anyhow::Result<(String, bool)>>>,
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
        let wasi = WasiCtxBuilder::new()
            .preopened_dir(
                Dir::from_std_file(File::open("./wasm/target/wasm32-wasi/wasi-deps/usr").unwrap()),
                DirPerms::READ,
                FilePerms::READ,
                "/usr",
            )
            .build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                limiter,
                client: reqwest::Client::new(),
                wasi,
                table: ResourceTable::new(),
                output: None,
            },
        );
        s.limiter(|s| &mut s.limiter);
        s
    }

    pub const fn reuse(&self) -> bool {
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
}
