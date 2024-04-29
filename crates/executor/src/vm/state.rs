use std::{path::Path, sync::Arc};

use anyhow::anyhow;
use parking_lot::Mutex;
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::mpsc;
use tracing::event;
use wasmtime::{
    component::{Linker, ResourceTable},
    Engine, Store,
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiView};

use crate::{
    atomic_cell::AtomicCell,
    resource::MemoryLimiter,
    trace::TracerContext,
    trace_output::TraceOutput,
    vm::{bindgen, bindgen::host_api::LogLevel, host_types, http_client, Sandbox},
    ExecStreamItem,
};

pub struct VmRunState {
    pub(crate) output: mpsc::Sender<ExecStreamItem>,
}

pub struct VmState {
    limiter: MemoryLimiter,
    client: ClientWithMiddleware,
    wasi: Mutex<WasiCtx>,
    table: Mutex<ResourceTable>,
    pub(crate) tracer: Arc<TracerContext>,
    pub(crate) run: Option<VmRunState>,
}

impl VmState {
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::add_to_linker_async(&mut linker)?;
        Sandbox::add_to_linker(&mut linker, |v: &mut Self| v)?;
        Ok(linker)
    }

    pub fn new(
        engine: &Engine,
        workdir: &Path,
        max_memory: usize,
        client: ClientWithMiddleware,
    ) -> Store<Self> {
        let tracer = Arc::new(AtomicCell::empty());
        let wasi = WasiCtxBuilder::new()
            .preopened_dir(
                "./wasm/target/wasm32-wasi/wasi-deps/usr",
                "/usr",
                DirPerms::READ,
                FilePerms::READ,
            )
            .unwrap()
            .preopened_dir(workdir, "/workdir", DirPerms::READ, FilePerms::READ)
            .unwrap()
            .stdout(TraceOutput::new(tracer.clone(), "stdout"))
            .stderr(TraceOutput::new(tracer.clone(), "stderr"))
            .build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                tracer,
                limiter,
                client,
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
impl bindgen::host_api::Host for VmState {
    async fn emit(&mut self, data: Vec<u8>) -> wasmtime::Result<()> {
        if let Some(run) = &self.run {
            run.output.send(ExecStreamItem::Data(data)).await?;
            Ok(())
        } else {
            Err(anyhow!("output channel missing"))
        }
    }

    async fn emit_log(&mut self, log_level: LogLevel, data: String) -> wasmtime::Result<()> {
        match log_level {
            LogLevel::Debug => event!(
                tracing::Level::DEBUG,
                promptkit.kind = "log",
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Info => event!(
                tracing::Level::INFO,
                promptkit.kind = "log",
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Warn => event!(
                tracing::Level::WARN,
                promptkit.kind = "log",
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Error => event!(
                tracing::Level::ERROR,
                promptkit.kind = "log",
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
        };

        self.tracer
            .with_async(|t| {
                t.log(
                    match log_level {
                        LogLevel::Debug => "debug",
                        LogLevel::Info => "info",
                        LogLevel::Warn => "warn",
                        LogLevel::Error => "error",
                    },
                    data.into(),
                )
            })
            .await;
        Ok(())
    }
}

impl http_client::HttpClientCtx for VmState {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    fn client(&self) -> &ClientWithMiddleware {
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
