use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::anyhow;
use bytes::Bytes;
use parking_lot::Mutex;
use rand::Rng;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use wasmtime::{
    component::{Component, Linker},
    Config, Engine, Store,
};
use wasmtime_wasi::preview2::{
    HostOutputStream, StdoutStream, StreamError, StreamResult, Subscribe, Table, WasiCtx,
    WasiCtxBuilder, WasiView,
};

wasmtime::component::bindgen!({
    world: "python-vm",
    async: true,
});

const MAX_MEMORY: u64 = 64 * 1024 * 1024;
const MAX_LOAD_FUEL: u64 = 5 * 1000 * 1000 * 1000;
const MAX_EXEC_FUEL: u64 = 5 * 1000 * 1000 * 1000;

struct SimpleState {
    wasi: WasiCtx,
    table: Table,
}

pub struct VmManager {
    engine: Engine,
    component: Component,
    caches: Mutex<HashMap<[u8; 32], Vec<Vm>>>,
}

#[derive(serde::Serialize)]
pub struct ExecResult {
    pub stdout: String,
    pub result: Box<RawValue>,
}

impl VmManager {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .consume_fuel(true)
            .static_memory_forced(true)
            .static_memory_maximum_size(MAX_MEMORY)
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
            caches: Mutex::new(HashMap::new()),
        })
    }

    pub async fn create(&self) -> anyhow::Result<Vm> {
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;

        let table: Table = Table::new();
        let stdout = MemoryOutput {
            buffer: MemoryOutputBuffer {
                capacity: 32 * 1024 * 1024,
                buffer: Arc::new(Mutex::new(Vec::new())),
            },
        };
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdout(stdout.clone());
        let wasi = wasi.build();

        let mut store = Store::new(&self.engine, SimpleState { wasi, table });
        let (bindings, _) =
            PythonVm::instantiate_async(&mut store, &self.component, &linker).await?;

        Ok(Vm {
            store,
            stdout,
            python: bindings,
        })
    }

    async fn put_cache(&self, hash: [u8; 32], vm: Vm) {
        let mut caches = self.caches.lock();
        caches.entry(hash).or_default().push(vm);

        let total = caches.values().map(|v| v.len()).sum::<usize>();
        if total > 64 {
            // remove random
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

    pub async fn exec(
        &self,
        script: &str,
        func: &str,
        args: &[String],
    ) -> anyhow::Result<ExecResult> {
        let mut hasher = Sha256::new();
        hasher.update(script);
        let hash: [u8; 32] = hasher.finalize().into();

        let vm = {
            let mut cache = self.caches.lock();
            cache.get_mut(&hash).and_then(|f| f.pop())
        };

        if let Some(mut vm) = vm {
            vm.store.set_fuel(MAX_EXEC_FUEL)?;
            if let Ok(s) = vm
                .python
                .python_vm()
                .call_call_func(&mut vm.store, func, args)
                .await
            {
                let r = ExecResult {
                    stdout: vm.stdout.pop(),
                    result: RawValue::from_string(s.map_err(|e| anyhow!(e))?)?,
                };
                self.put_cache(hash, vm).await;
                return Ok(r);
            }
        }

        let mut vm = self.create().await?;
        vm.store.set_fuel(MAX_LOAD_FUEL)?;
        vm.python
            .python_vm()
            .call_eval_script(&mut vm.store, script)
            .await?
            .map_err(|e| anyhow!(e))?;

        vm.store.set_fuel(MAX_EXEC_FUEL)?;
        let ret = vm
            .python
            .python_vm()
            .call_call_func(&mut vm.store, func, args)
            .await?
            .map_err(|e| anyhow!(e))?;

        let r = ExecResult {
            stdout: vm.stdout.pop(),
            result: RawValue::from_string(ret)?,
        };
        self.put_cache(hash, vm).await;
        Ok(r)
    }
}

#[derive(Clone)]
pub struct MemoryOutput {
    buffer: MemoryOutputBuffer,
}

impl MemoryOutput {
    pub fn pop(&self) -> String {
        let mut buf = self.buffer.buffer.lock();
        let out = String::from_utf8_lossy(&buf).into_owned();
        buf.clear();
        out
    }
}

#[derive(Clone)]
pub struct MemoryOutputBuffer {
    capacity: usize,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl StdoutStream for MemoryOutput {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.buffer.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

impl HostOutputStream for MemoryOutputBuffer {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let mut buf = self.buffer.lock();
        if bytes.len() > self.capacity - buf.len() {
            return Err(StreamError::Trap(anyhow!(
                "write beyond capacity of MemoryOutputPipe"
            )));
        }
        buf.extend_from_slice(bytes.as_ref());
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        let consumed = self.buffer.lock().len();
        if consumed < self.capacity {
            Ok(self.capacity - consumed)
        } else {
            Err(StreamError::Closed)
        }
    }
}

#[async_trait::async_trait]
impl Subscribe for MemoryOutputBuffer {
    async fn ready(&mut self) {}
}

pub struct Vm {
    store: Store<SimpleState>,
    stdout: MemoryOutput,
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
