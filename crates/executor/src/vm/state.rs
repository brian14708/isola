use std::{path::Path, sync::Arc};

use anyhow::anyhow;
use parking_lot::Mutex;
use promptkit_llm::tokenizers::Tokenizer;
use tokio::sync::mpsc;
use wasmtime::{
    component::{Linker, ResourceTable},
    Engine, Store,
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiView};

use crate::{
    resource::MemoryLimiter, trace_output::TraceOutput, wasm::vm::VmView, Env, ExecStreamItem,
};

use crate::wasm::{http::HttpView, llm::LlmView};

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

impl<E: Env + Send> VmState<E> {
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::add_to_linker_async(&mut linker)?;
        crate::wasm::http::add_to_linker(&mut linker)?;
        crate::wasm::llm::add_to_linker(&mut linker)?;
        crate::wasm::vm::add_to_linker(&mut linker)?;
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

impl<E: Send> WasiView for VmState<E> {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        self.wasi.get_mut()
    }
}

impl<E: Env + Send> LlmView for VmState<E> {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    async fn get_tokenizer(&mut self, name: &str) -> Option<Arc<dyn Tokenizer + Send + Sync>> {
        self.env.get_tokenizer(name).await.ok()
    }
}

impl<E: Env + Send> HttpView for VmState<E> {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    fn send_request(
        &mut self,
        req: reqwest::Request,
    ) -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send + 'static
    {
        self.env.send_request(req)
    }
}

impl<E: Send> VmView for VmState<E> {
    fn table(&mut self) -> &mut ResourceTable {
        self.table.get_mut()
    }

    async fn emit(&mut self, data: Vec<u8>) -> wasmtime::Result<()> {
        if let Some(run) = &self.run {
            run.output.send(ExecStreamItem::Data(data)).await?;
            Ok(())
        } else {
            Err(anyhow!("output channel missing"))
        }
    }
}
