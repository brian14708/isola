use anyhow::anyhow;
use tokio::sync::mpsc;
use wasmtime::{Engine, Store};
use wasmtime_wasi::preview2::{Table, WasiCtx, WasiCtxBuilder, WasiView};

use crate::resource::MemoryLimiter;

use self::host::Host;

wasmtime::component::bindgen!({
    world: "python-vm",
    async: true,
});

pub struct VmState {
    limiter: MemoryLimiter,
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
impl Host for VmState {
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
