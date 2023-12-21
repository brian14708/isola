use std::{path::Path};

use anyhow::anyhow;
use wasmtime::{
    component::{Component, Linker},
    Config, Engine, Store,
};
use wasmtime_wasi::preview2::{Table, WasiCtx, WasiCtxBuilder, WasiView};

pub struct ScriptRunner {
    engine: Engine,
    component: Component,
}

wasmtime::component::bindgen!({
    world: "python-executor",
    async: true,
});

struct SimpleState {
    wasi: WasiCtx,
    table: Table,
}

impl ScriptRunner {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .async_support(true)
            .cranelift_opt_level(wasmtime::OptLevel::Speed);
        let engine = Engine::new(&config)?;
        println!("Loading module...");
        let component =
            match unsafe { Component::deserialize_file(&engine, Path::new(".cache.wasm")) } {
                Ok(c) => c,
                Err(_) => {
                    let component = Component::from_file(&engine, path)?;
                    let data = component.serialize()?;
                    std::fs::write(".cache.wasm", data)?;
                    component
                }
            };
        println!("Loaded module!");

        Ok(Self { engine, component })
    }

    pub async fn create_script(&self) -> anyhow::Result<Script> {
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;

        let table = Table::new();
        let mut wasi = WasiCtxBuilder::new();
        wasi.inherit_stdio();
        let wasi = wasi.build();

        let mut store = Store::new(&self.engine, SimpleState { wasi, table });
        let (bindings, _) =
            PythonExecutor::instantiate_async(&mut store, &self.component, &linker).await?;

        // let vm = bindings
        //     .python_executor()
        //     .script()
        //     .call_constructor(&mut store)
        //     .await?;

        Ok(Script {
            store,
            python: bindings,
            // vm,
        })
    }
}

pub struct Script {
    store: Store<SimpleState>,
    python: PythonExecutor,
    // vm: ResourceAny,
}

impl Script {
    pub async fn run(
        &mut self,
        script: &str,
        func: String,
        args: Vec<String>,
    ) -> anyhow::Result<String> {
        let ret = self
            .python
            .python_executor()
            .call_run_script(&mut self.store, script, &func, &args)
            .await?
            .map_err(|e| anyhow!(e))?;
        Ok(ret)
    }
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
