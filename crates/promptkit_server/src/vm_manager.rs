use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use anyhow::anyhow;
use axum::{
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    Json,
};
use parking_lot::Mutex;
use rand::Rng;
use serde_json::{json, value::RawValue};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use wasmtime::{
    component::{Component, Linker},
    Config, Engine, Store,
};
use wasmtime_wasi::preview2::{Table, WasiCtx, WasiCtxBuilder, WasiView};

use crate::resource::MemoryLimiter;

use self::host::Host;

wasmtime::component::bindgen!({
    world: "python-vm",
    async: true,
});

const MAX_MEMORY: usize = 64 * 1024 * 1024;
const MAX_LOAD_FUEL: u64 = 1000 * 1000 * 1000;
const MAX_EXEC_FUEL: u64 = 5 * 1000 * 1000 * 1000;

struct SimpleState {
    host_env: HostEnv,
    limiter: MemoryLimiter,
    wasi: WasiCtx,
    table: Table,
}

pub struct VmManager {
    engine: Engine,
    component: Component,
    caches: Arc<Mutex<HashMap<[u8; 32], Vec<Vm>>>>,
}

impl VmManager {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .consume_fuel(true)
            .cranelift_opt_level(wasmtime::OptLevel::Speed);
        let engine = Engine::new(&config)?;
        println!("Loading module...");
        let component =
            match unsafe { Component::deserialize_file(&engine, Path::new(".cache.wasm")) } {
                Ok(c) => c,
                Err(_) => {
                    let component = Component::from_file(&engine, path)?;
                    #[cfg(debug_assertions)]
                    {
                        let data = component.serialize()?;
                        std::fs::write(".cache.wasm", data)?;
                    }
                    component
                }
            };
        println!("Loaded module!");

        Ok(Self {
            engine,
            component,
            caches: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn create(&self) -> anyhow::Result<Vm> {
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;

        let table: Table = Table::new();
        let mut wasi = WasiCtxBuilder::new();
        let wasi = wasi.build();
        let m = MemoryLimiter::new(MAX_MEMORY / 2, MAX_MEMORY);

        let host_env = HostEnv { output: None };
        let mut store = Store::new(
            &self.engine,
            SimpleState {
                host_env,
                limiter: m,
                wasi,
                table,
            },
        );
        store.limiter(|s| &mut s.limiter);
        host::add_to_linker(&mut linker, |v: &mut SimpleState| &mut v.host_env)?;

        let (bindings, _) =
            PythonVm::instantiate_async(&mut store, &self.component, &linker).await?;

        Ok(Vm {
            store,
            python: bindings,
        })
    }

    async fn put_cache(cache: Arc<Mutex<HashMap<[u8; 32], Vec<Vm>>>>, hash: [u8; 32], vm: Vm) {
        if vm.store.data().limiter.exceed_soft() {
            return;
        }

        let mut caches = cache.lock();
        caches.entry(hash).or_default().push(vm);

        let total = caches.values().map(|v| v.len()).sum::<usize>();
        if total > 64 {
            let mut rng = rand::thread_rng();
            let rm_idx = rng.gen_range(0..total);

            let mut idx = 0;
            let mut rm_key = None;
            for (k, v) in caches.iter_mut() {
                if idx + v.len() > rm_idx {
                    v.pop();
                    if v.is_empty() {
                        rm_key = Some(*k);
                    }
                    break;
                }
                idx += v.len();
            }
            if let Some(k) = rm_key {
                caches.remove(&k);
            }
        }
    }

    async fn exec_impl(
        &self,
        func: &str,
        args: &[String],
        hash: [u8; 32],
        mut vm: Vm,
    ) -> anyhow::Result<Response> {
        let (tx, mut rx) = mpsc::channel(4);
        vm.store.data_mut().host_env.output = Some(tx.clone());
        vm.store.set_fuel(MAX_EXEC_FUEL)?;
        let func = func.to_string();
        let args = args.to_vec();
        let cache = Arc::downgrade(&self.caches);
        let exec = tokio::task::spawn(async move {
            let ret = vm
                .python
                .python_vm()
                .call_call_func(&mut vm.store, &func, &args)
                .await
                .and_then(|v| v.map_err(|e| anyhow!(e)));
            vm.store.data_mut().host_env.output = None;
            match ret {
                Ok(()) => {
                    if let Some(cache) = cache.upgrade() {
                        Self::put_cache(cache, hash, vm).await;
                    }
                }
                Err(err) => {
                    _ = tx.send(Err(err)).await;
                }
            };
        });

        let data = match rx.recv().await {
            Some(Ok((data, true))) => {
                exec.await?;
                return match rx.recv().await {
                    Some(Ok(_)) => Err(anyhow!("unexpected")),
                    Some(Err(err)) => Ok((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "error": err.to_string(),
                        })),
                    )
                        .into_response()),
                    None => {
                        Ok((StatusCode::OK, Json(RawValue::from_string(data)?)).into_response())
                    }
                };
            }
            Some(Ok((data, false))) => data,
            Some(Err(err)) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": err.to_string(),
                    })),
                )
                    .into_response())
            }
            None => {
                return Err(anyhow!("unexpected error"));
            }
        };

        let stream = tokio_stream::iter([Ok((data, false))])
            .chain(ReceiverStream::new(rx))
            .map::<anyhow::Result<Event>, _>(|v| match v {
                Ok((data, true)) => {
                    Ok(Event::default().data(if data.is_empty() { "[DONE]" } else { &data }))
                }
                Ok((data, false)) => Ok(Event::default().data(data)),
                Err(err) => Ok(Event::default()
                    .event("error")
                    .json_data(json!({
                        "error": err.to_string(),
                    }))
                    .unwrap()),
            });

        Ok(Sse::new(stream)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(1))
                    .text("keepalive"),
            )
            .into_response())
    }

    pub async fn exec(
        &self,
        script: &str,
        func: &str,
        args: &[String],
    ) -> anyhow::Result<Response> {
        let mut hasher = Sha256::new();
        hasher.update(script);
        let hash: [u8; 32] = hasher.finalize().into();

        let vm = {
            let mut cache = self.caches.lock();
            cache.get_mut(&hash).and_then(|f| f.pop())
        };

        if let Some(mut vm) = vm {
            vm.store.set_fuel(MAX_EXEC_FUEL)?;
            return self.exec_impl(func, args, hash, vm).await;
        }

        let mut vm = self.create().await?;
        vm.store.set_fuel(MAX_LOAD_FUEL)?;
        vm.python
            .python_vm()
            .call_eval_script(&mut vm.store, script)
            .await?
            .map_err(|e| anyhow!(e))?;

        self.exec_impl(func, args, hash, vm).await
    }
}

pub struct Vm {
    store: Store<SimpleState>,
    python: PythonVm,
}

impl WasiView for SimpleState {
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

pub struct HostEnv {
    output: Option<mpsc::Sender<anyhow::Result<(String, bool)>>>,
}

#[async_trait::async_trait]
impl Host for HostEnv {
    async fn emit(&mut self, data: String, end: bool) -> wasmtime::Result<()> {
        if let Some(output) = &self.output {
            output.send(Ok((data, end))).await?;
            Ok(())
        } else {
            Err(anyhow!("output channel missing"))
        }
    }
}
