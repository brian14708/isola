use std::path::Path;

use component_init_transform::Invoker;
use futures::FutureExt;
use wasmtime::{
    Engine, Store,
    component::{Component, Instance as WasmInstance},
};

use crate::{
    host::{BoxError, Host, HttpRequest, HttpResponse},
    internal::{
        module::{
            ModuleConfig,
            cache::{cache_key, write_cache_file_atomic},
        },
        sandbox::{InstanceState, exports::GuestIndices},
    },
    sandbox::{DirectoryMapping, Error, Result},
    value::Value as IsolaValue,
};

pub async fn load_or_compile_component(
    engine: &Engine,
    wasm_path: &Path,
    directory_mappings: &[DirectoryMapping],
    cfg: &ModuleConfig,
) -> Result<Component> {
    let wasm_bytes = tokio::fs::read(wasm_path).await.map_err(Error::from)?;

    let Some(cache_dir) = &cfg.cache else {
        let bytes =
            compile_serialized_component(engine, cfg, directory_mappings, &wasm_bytes).await?;
        // SAFETY: bytes are produced by wasmtime for the same version/config; if
        // incompatible, deserialization will fail and surface as an error.
        let component = unsafe { Component::deserialize(engine, &bytes) }.map_err(Error::Wasm)?;
        return Ok(component);
    };

    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(Error::from)?;
    let key = cache_key(engine, cfg, &wasm_bytes);
    let cache_path = cache_dir.join(format!("{key}.cwasm"));

    if let Ok(component) = unsafe { Component::deserialize_file(engine, &cache_path) } {
        return Ok(component);
    }

    let bytes = compile_serialized_component(engine, cfg, directory_mappings, &wasm_bytes).await?;
    write_cache_file_atomic(&cache_path, &bytes).await?;

    let component =
        unsafe { Component::deserialize_file(engine, &cache_path) }.map_err(Error::Wasm)?;
    Ok(component)
}

async fn compile_serialized_component(
    engine: &Engine,
    cfg: &ModuleConfig,
    directory_mappings: &[DirectoryMapping],
    wasm_bytes: &[u8],
) -> Result<Vec<u8>> {
    let engine = engine.clone();
    let cfg = cfg.clone();
    let directory_mappings = directory_mappings.to_vec();
    let wasm_bytes = wasm_bytes.to_vec();

    tokio::task::spawn_blocking(move || {
        // Run initialization on a blocking worker so non-Send internals from
        // component-init-transform do not leak into the outer async future type.
        tokio::runtime::Handle::current().block_on(async move {
            let data = component_init_transform::initialize(&wasm_bytes, |instrumented| {
                let engine = engine.clone();
                async move {
                    let component = Component::new(&engine, &instrumented).map_err(Error::Wasm)?;

                    let linker =
                        InstanceState::<CompileHost>::new_linker(&engine).map_err(Error::Wasm)?;
                    let mut store = InstanceState::new(
                        &engine,
                        &directory_mappings,
                        &cfg.env,
                        cfg.max_memory,
                        CompileHost,
                    )
                    .map_err(Error::Wasm)?;
                    store.epoch_deadline_async_yield_and_update(1);

                    let pre = linker.instantiate_pre(&component).map_err(Error::Wasm)?;
                    let binding = pre
                        .instantiate_async(&mut store)
                        .await
                        .map_err(Error::Wasm)?;
                    let guest = GuestIndices::new(&pre)
                        .map_err(Error::Wasm)?
                        .load(&mut store, &binding)
                        .map_err(Error::Wasm)?;

                    guest
                        .call_initialize(&mut store, true, cfg.prelude.as_deref())
                        .await
                        .map_err(Error::Wasm)?;

                    Ok(Box::new(MyInvoker {
                        store,
                        instance: binding,
                    }) as Box<dyn Invoker>)
                }
                .boxed()
            })
            .await
            .map_err(|e| Error::Other(e.into()))?;

            let component = Component::new(&engine, &data).map_err(Error::Wasm)?;
            component.serialize().map_err(Error::Wasm)
        })
    })
    .await
    .map_err(|e| Error::Other(e.into()))?
}

// Helper structs for compilation pre-init.

struct MyInvoker<S: Host> {
    store: Store<InstanceState<S>>,
    instance: WasmInstance,
}

#[async_trait::async_trait]
impl<S: Host> Invoker for MyInvoker<S> {
    async fn call_s32(&mut self, function: &str) -> anyhow::Result<i32> {
        let func = self
            .instance
            .get_typed_func::<(), (i32,)>(&mut self.store, function)
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?;
        let result = func
            .call_async(&mut self.store, ())
            .await
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?
            .0;
        Ok(result)
    }

    async fn call_s64(&mut self, function: &str) -> anyhow::Result<i64> {
        let func = self
            .instance
            .get_typed_func::<(), (i64,)>(&mut self.store, function)
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?;
        let result = func
            .call_async(&mut self.store, ())
            .await
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?
            .0;
        Ok(result)
    }

    async fn call_f32(&mut self, function: &str) -> anyhow::Result<f32> {
        let func = self
            .instance
            .get_typed_func::<(), (f32,)>(&mut self.store, function)
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?;
        let result = func
            .call_async(&mut self.store, ())
            .await
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?
            .0;
        Ok(result)
    }

    async fn call_f64(&mut self, function: &str) -> anyhow::Result<f64> {
        let func = self
            .instance
            .get_typed_func::<(), (f64,)>(&mut self.store, function)
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?;
        let result = func
            .call_async(&mut self.store, ())
            .await
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?
            .0;
        Ok(result)
    }

    async fn call_list_u8(&mut self, function: &str) -> anyhow::Result<Vec<u8>> {
        let func = self
            .instance
            .get_typed_func::<(), (Vec<u8>,)>(&mut self.store, function)
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?;
        let result = func
            .call_async(&mut self.store, ())
            .await
            .map_err(|error| anyhow::Error::from_boxed(error.into()))?
            .0;
        Ok(result)
    }
}

struct CompileHost;

#[async_trait::async_trait]
impl Host for CompileHost {
    async fn hostcall(
        &self,
        _call_type: &str,
        _payload: IsolaValue,
    ) -> core::result::Result<IsolaValue, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }

    async fn http_request(
        &self,
        _req: HttpRequest,
    ) -> core::result::Result<HttpResponse, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }
}
