use std::{future::Future, path::Path};

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use tokio::time::timeout;
use tracing::Instrument;
use wasmtime::{
    Engine, Store,
    component::{Linker, ResourceTable},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    HttpError, HttpResult, WasiHttpCtx, WasiHttpView,
    bindings::http::outgoing_handler::ErrorCode,
    body::{HyperIncomingBody, HyperOutgoingBody},
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use super::bindgen::{HostView, add_to_linker};
use crate::{
    env::EnvHandle, resource::MemoryLimiter, trace_output::TraceOutput, vm::bindgen::EmitValue,
};

pub trait OutputCallback: Send {
    fn on_result(&mut self, item: Bytes) -> impl Future<Output = Result<(), anyhow::Error>> + Send;

    fn on_end(&mut self, item: Bytes) -> impl Future<Output = Result<(), anyhow::Error>> + Send;
}

pub struct VmRunState<E: EnvHandle> {
    pub(crate) env: E,
    pub(crate) output: E::Callback,
}

pub struct VmState<E: EnvHandle> {
    limiter: MemoryLimiter,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    pub(crate) run: Option<VmRunState<E>>,

    output_buffer: OutputBuffer,
}

impl<E: EnvHandle> VmState<E> {
    /// Creates a new linker for the VM state.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the WASI components fail to link.
    pub fn new_linker(engine: &Engine) -> anyhow::Result<Linker<Self>> {
        let mut linker = Linker::<Self>::new(engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        crate::wasm::logging::add_to_linker(&mut linker)?;
        add_to_linker(&mut linker)?;
        Ok(linker)
    }

    /// Creates a new VM state with the specified configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the preopened directories cannot be added to the WASI context.
    pub fn new(
        engine: &Engine,
        base_dir: &Path,
        workdir: &Path,
        max_memory: usize,
    ) -> anyhow::Result<Store<Self>> {
        let wasi = WasiCtxBuilder::new()
            .preopened_dir(base_dir, "/usr", DirPerms::READ, FilePerms::READ)
            .map_err(|e| anyhow::anyhow!("Failed to add base_dir to WASI context: {}", e))?
            .preopened_dir(workdir, "/workdir", DirPerms::READ, FilePerms::READ)
            .map_err(|e| anyhow::anyhow!("Failed to add workdir to WASI context: {}", e))?
            .allow_tcp(false)
            .allow_udp(false)
            .stdout(TraceOutput::new("stdout"))
            .stderr(TraceOutput::new("stderr"))
            .build();
        let limiter = MemoryLimiter::new(max_memory / 2, max_memory);

        let mut s = Store::new(
            engine,
            Self {
                limiter,
                wasi,
                http: WasiHttpCtx::new(),
                table: ResourceTable::new(),
                run: None,
                output_buffer: OutputBuffer::new(),
            },
        );
        s.limiter(|s| &mut s.limiter);
        Ok(s)
    }

    pub const fn reuse(&self) -> bool {
        !self.limiter.exceed_soft()
    }
}

impl<E: EnvHandle> WasiView for VmState<E> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl<E: EnvHandle> WasiHttpView for VmState<E> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let env = self.env().map_err(HttpError::trap)?;
        let handle = wasmtime_wasi::runtime::spawn(
            async move {
                let resp = timeout(config.first_byte_timeout, env.send_request_http(request));
                let (part, body) = match resp.await {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        return Ok(Err(ErrorCode::InternalError(Some(format!(
                            "request error: {}",
                            e.into()
                        )))));
                    }
                    Err(_) => return Ok(Err(ErrorCode::HttpResponseTimeout)),
                }
                .map(|b| {
                    http_body_util::StreamBody::new(b.map(|e| match e {
                        Ok(e) => Ok(e),
                        Err(e) => Err(ErrorCode::InternalError(Some(
                            Into::<anyhow::Error>::into(e).to_string(),
                        ))),
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
            }
            .in_current_span(),
        );
        Ok(HostFutureIncomingResponse::pending(handle))
    }
}

impl<E: EnvHandle> HostView for VmState<E> {
    type Env = E;

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn env(&mut self) -> wasmtime::Result<Self::Env> {
        let Some(run) = self.run.as_mut() else {
            return Err(anyhow::anyhow!("env missing"));
        };
        Ok(run.env.clone())
    }

    async fn emit(&mut self, data: EmitValue) -> wasmtime::Result<()> {
        let Some(run) = self.run.as_mut() else {
            return Err(anyhow::anyhow!("output channel missing"));
        };

        match data {
            EmitValue::Continuation(new_data) => {
                self.output_buffer.append(new_data);
            }
            EmitValue::End(new_data) => {
                self.output_buffer.append(new_data);
                let output = self.output_buffer.take();
                run.output.on_end(output).await?;
            }
            EmitValue::PartialResult(new_data) => {
                self.output_buffer.append(new_data);
                let output = self.output_buffer.take();
                run.output.on_result(output).await?;
            }
        }
        Ok(())
    }
}

struct OutputBuffer(BytesMut);

impl OutputBuffer {
    fn new() -> Self {
        Self(BytesMut::new())
    }

    #[inline]
    fn append(&mut self, data: Bytes) {
        self.0.extend(data);
    }

    #[inline]
    fn take(&mut self) -> Bytes {
        std::mem::take(&mut self.0).freeze()
    }
}
