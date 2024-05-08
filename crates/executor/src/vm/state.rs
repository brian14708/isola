use std::{path::Path, sync::Arc};

use anyhow::anyhow;
use parking_lot::Mutex;
use promptkit_llm::tokenizers::Tokenizer;
use tokio::sync::mpsc;
use tracing::event;
use wasmtime::{
    component::{Linker, ResourceTable},
    Engine, Store,
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiView};

use crate::{
    env::EnvError,
    resource::MemoryLimiter,
    trace_output::TraceOutput,
    vm::{
        bindgen::{self, host_api::LogLevel},
        host_types, Sandbox,
    },
    Env, ExecStreamItem,
};

pub struct VmRunState {
    pub(crate) output: mpsc::Sender<ExecStreamItem>,
}

pub struct VmState<E> {
    limiter: MemoryLimiter,
    env: E,
    wasi: Mutex<WasiCtx>,
    table: Mutex<ResourceTable>,
    pub(crate) run: Option<VmRunState>,
}

impl<E> VmState<E>
where
    E: Env + Sync + Send,
{
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::add_to_linker_async(&mut linker)?;
        Sandbox::add_to_linker(&mut linker, |v: &mut Self| v)?;
        Ok(linker)
    }

    pub fn new(engine: &Engine, workdir: &Path, max_memory: usize, env: E) -> Store<Self> {
        let wasi = WasiCtxBuilder::new()
            .preopened_dir(
                "./wasm/target/wasm32-wasip1/wasi-deps/usr",
                "/usr",
                DirPerms::READ,
                FilePerms::READ,
            )
            .unwrap()
            .preopened_dir(workdir, "/workdir", DirPerms::READ, FilePerms::READ)
            .unwrap()
            .stdout(TraceOutput::new("stdout"))
            .stderr(TraceOutput::new("stderr"))
            .build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                limiter,
                env,
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

impl<E> WasiView for VmState<E>
where
    E: Send,
{
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        self.wasi.get_mut()
    }
}

#[async_trait::async_trait]
impl<E> bindgen::host_api::Host for VmState<E>
where
    E: Env + Sync + Send,
{
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
                target: "promptkit::debug",
                tracing::Level::DEBUG,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Info => event!(
                target: "promptkit::info",
                tracing::Level::INFO,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Warn => event!(
                target: "promptkit::warn",
                tracing::Level::WARN,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
            LogLevel::Error => event!(
                target: "promptkit::error",
                tracing::Level::ERROR,
                promptkit.log.output = &data,
                promptkit.user = true,
            ),
        };
        Ok(())
    }
}

impl<E> host_types::HostTypesCtx for VmState<E>
where
    E: Send,
{
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }
}

pub trait EnvCtx: Env + Send {
    fn table(&mut self) -> &mut ResourceTable;
}

impl<E> EnvCtx for VmState<E>
where
    E: Env + Send + Sync,
{
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }
}

impl<E> Env for VmState<E>
where
    E: Env + Sync,
{
    fn hash(&self, _update: impl FnMut(&[u8])) {
        unreachable!("hashing not implemented")
    }

    async fn send_request(&self, req: reqwest::Request) -> Result<reqwest::Response, EnvError> {
        self.env.send_request(req).await
    }

    async fn get_tokenizer(
        &self,
        name: &str,
    ) -> Result<Arc<dyn Tokenizer + Send + Sync>, EnvError> {
        self.env.get_tokenizer(name).await
    }
}
