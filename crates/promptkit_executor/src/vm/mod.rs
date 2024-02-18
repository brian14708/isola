mod bindgen;
mod http_client;

use std::fs::File;
use std::future::Future;

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
use crate::trace::TraceLogLevel;
use crate::trace::Tracer;
use crate::trace_output::TraceContext;
use crate::trace_output::TraceOutput;

struct VmRunState {
    output: mpsc::Sender<anyhow::Result<(String, bool)>>,
}

pub struct VmState {
    tracer_ctx: TraceContext,
    limiter: MemoryLimiter,
    client: reqwest::Client,
    wasi: WasiCtx,
    table: ResourceTable,
    run: Option<VmRunState>,
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
        let ctx = TraceContext::new();
        let wasi = WasiCtxBuilder::new()
            .preopened_dir(
                Dir::from_std_file(File::open("./wasm/target/wasm32-wasi/wasi-deps/usr").unwrap()),
                DirPerms::READ,
                FilePerms::READ,
                "/usr",
            )
            .stdout(TraceOutput::new(&ctx, TraceLogLevel::Stdout))
            .stderr(TraceOutput::new(&ctx, TraceLogLevel::Stderr))
            .build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                tracer_ctx: ctx,
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

pub struct Vm {
    pub(crate) hash: [u8; 32],
    pub(crate) store: Store<VmState>,
    pub(crate) python: PythonVm,
}

pub(crate) struct VmRun {
    vm: Vm,
}

impl VmRun {
    pub fn new(
        mut vm: Vm,
        tracer: Option<impl Tracer>,
        sender: mpsc::Sender<anyhow::Result<(String, bool)>>,
    ) -> Self {
        let o: &mut VmState = vm.store.data_mut();
        if let Some(tracer) = tracer {
            o.tracer_ctx.set(Some(tracer.boxed_logger()));
        }
        o.run = Some(VmRunState { output: sender });
        Self { vm }
    }

    pub async fn exec<'a, F, Output>(
        &'a mut self,
        f: impl FnOnce(&'a exports::Vm, &'a mut Store<VmState>) -> F,
    ) -> Output
    where
        F: Future<Output = Output>,
    {
        let vm = self.vm.python.vm();
        let store = &mut self.vm.store;
        f(vm, store).await
    }

    pub fn finalize(mut self) -> Vm {
        let o: &mut VmState = self.vm.store.data_mut();
        o.run = None;
        o.tracer_ctx.set(None);
        self.vm
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
}
