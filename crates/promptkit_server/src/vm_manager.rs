use std::{path::Path, time::Duration};

use anyhow::anyhow;
use axum::{
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    Json,
};

use serde_json::{json, value::RawValue};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::info;
use wasmtime::{
    component::{Component, InstancePre, Linker},
    Config, Engine,
};

use crate::{
    vm::{host, PythonVm, Vm, VmState},
    vm_cache::VmCache,
};

const MAX_MEMORY: usize = 64 * 1024 * 1024;
const MAX_LOAD_FUEL: u64 = 1000 * 1000 * 1000;
const MAX_EXEC_FUEL: u64 = 5 * 1000 * 1000 * 1000;

pub struct VmManager {
    engine: Engine,
    instance_pre: InstancePre<VmState>,
    cache: VmCache,
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

        info!("Loading module...");
        #[cfg(debug_assertions)]
        let component = match unsafe {
            Component::deserialize_file(&engine, path.with_extension("wasm.cache"))
        } {
            Ok(c) => c,
            Err(_) => {
                let component = Component::from_file(&engine, path)?;
                let data = component.serialize()?;
                std::fs::write(path.with_extension("wasm.cache"), data)?;
                component
            }
        };

        #[cfg(not(debug_assertions))]
        let component = Component::from_file(&engine, path)?;

        let mut linker = Linker::<VmState>::new(&engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;
        host::add_to_linker(&mut linker, |v: &mut VmState| v)?;
        let instance_pre = linker.instantiate_pre(&component)?;

        info!("Loaded module!");

        Ok(Self {
            engine,
            instance_pre,
            cache: VmCache::new(),
        })
    }

    pub async fn create(&self, hash: [u8; 32]) -> anyhow::Result<Vm> {
        let mut store = VmState::new(&self.engine, MAX_MEMORY);
        let (bindings, _) = PythonVm::instantiate_pre(&mut store, &self.instance_pre).await?;
        Ok(Vm {
            hash,
            store,
            python: bindings,
        })
    }

    async fn exec_impl(
        &self,
        func: String,
        args: Vec<String>,
        mut vm: Vm,
    ) -> anyhow::Result<Response> {
        let (tx, mut rx) = mpsc::channel(4);
        vm.store.data_mut().initialize(tx.clone());
        vm.store.set_fuel(MAX_EXEC_FUEL)?;

        let cache_weak = self.cache.downgrade();
        let exec = tokio::task::spawn(async move {
            let ret = vm
                .python
                .python_vm()
                .call_call_func(&mut vm.store, &func, &args)
                .await
                .and_then(|v| v.map_err(|e| anyhow!(e)));
            match ret {
                Ok(()) => {
                    cache_weak.put(vm);
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
        func: String,
        args: Vec<String>,
    ) -> anyhow::Result<Response> {
        let mut hasher = Sha256::new();
        hasher.update(script);
        let hash: [u8; 32] = hasher.finalize().into();

        let vm = self.cache.get(hash);
        if let Some(vm) = vm {
            return self.exec_impl(func, args, vm).await;
        }

        let mut vm = self.create(hash).await?;
        vm.store.set_fuel(MAX_LOAD_FUEL)?;
        vm.python
            .python_vm()
            .call_eval_script(&mut vm.store, script)
            .await?
            .map_err(|e| anyhow!(e))?;

        self.exec_impl(func, args, vm).await
    }
}
