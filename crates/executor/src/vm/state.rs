use std::path::Path;

use futures_util::StreamExt;
use tokio::{sync::mpsc, time::timeout};
use wasmtime::{
    Engine, Store,
    component::{Linker, ResourceTable},
};
use wasmtime_wasi::{DirPerms, FilePerms, IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{
    HttpResult, WasiHttpCtx, WasiHttpView,
    bindings::http::outgoing_handler::ErrorCode,
    body::{HyperIncomingBody, HyperOutgoingBody},
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use crate::{Env, ExecStreamItem, resource::MemoryLimiter, trace_output::TraceOutput};

use super::bindgen::{HostView, add_to_linker};

pub struct VmRunState {
    pub(crate) output: mpsc::Sender<ExecStreamItem>,
}

pub struct VmState<E> {
    limiter: MemoryLimiter,
    env: E,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    pub(crate) run: Option<VmRunState>,
}

impl<E: Env + Send> VmState<E> {
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        crate::wasm::logging::add_to_linker(&mut linker)?;
        add_to_linker(&mut linker)?;
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
                wasi,
                http: WasiHttpCtx::new(),
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

impl<E: Send> IoView for VmState<E> {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl<E: Send> WasiView for VmState<E> {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl<E: Env + Send> WasiHttpView for VmState<E> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let resp = timeout(
            config.first_byte_timeout,
            self.env.send_request_http(request),
        );

        let handle = wasmtime_wasi::runtime::spawn(async move {
            let (part, body) = match resp.await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    return Ok(Err(ErrorCode::InternalError(Some(format!(
                        "request error: {e}"
                    )))));
                }
                Err(_) => return Ok(Err(ErrorCode::HttpResponseTimeout)),
            }
            .map(|b| {
                http_body_util::StreamBody::new(b.map(|e| match e {
                    Ok(e) => Ok(e),
                    Err(e) => Err(ErrorCode::InternalError(Some(e.to_string()))),
                }))
            })
            .into_parts();
            Ok(Ok(IncomingResponse {
                resp: hyper::Response::<HyperIncomingBody>::from_parts(
                    part,
                    HyperIncomingBody::new(body),
                ),
                worker: None,
                between_bytes_timeout: config.between_bytes_timeout,
            }))
        });
        Ok(HostFutureIncomingResponse::pending(handle))
    }
}

impl<E: Send + Env> HostView for VmState<E> {
    type Env = E;

    fn env(&mut self) -> &mut Self::Env {
        &mut self.env
    }

    async fn emit(&mut self, data: Vec<u8>) -> wasmtime::Result<()> {
        match &self.run {
            Some(run) => {
                run.output.send(ExecStreamItem::Data(data)).await?;
                Ok(())
            }
            _ => Err(anyhow::anyhow!("output channel missing")),
        }
    }
}
