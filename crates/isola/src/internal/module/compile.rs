use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use wasmtime::{Engine, component::Component};
use wasmtime_wizer::{WasmtimeWizerComponent, Wizer};

use crate::{
    host::{BoxError, Host, HttpRequestStream, HttpResponse},
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
        // SAFETY: bytes are produced by wasmtime for the same version/config;
        // if incompatible, deserialization will fail and surface as an
        // error.
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

    let lock = cache_lock(&cache_path);
    let _guard = lock.lock().await;

    // Another task may have populated the cache while this task waited for
    // the per-key lock.
    if let Ok(component) = unsafe { Component::deserialize_file(engine, &cache_path) } {
        return Ok(component);
    }

    let bytes = compile_serialized_component(engine, cfg, directory_mappings, &wasm_bytes).await?;
    write_cache_file_atomic(&cache_path, &bytes).await?;

    let component =
        unsafe { Component::deserialize_file(engine, &cache_path) }.map_err(Error::Wasm)?;
    Ok(component)
}

fn cache_lock(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().expect("cache lock registry poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(path.to_owned(), Arc::downgrade(&lock));
    lock
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
        // Run initialization on a blocking worker so the compile-time guest
        // instantiation stays off the main async scheduler.
        tokio::runtime::Handle::current().block_on(async move {
            let wizer = Wizer::new();
            let (cx, instrumented_wasm) = wizer
                .instrument_component(&wasm_bytes)
                .map_err(Error::Wasm)?;

            let component = Component::new(&engine, &instrumented_wasm).map_err(Error::Wasm)?;
            let linker = InstanceState::<CompileHost>::new_linker(&engine).map_err(Error::Wasm)?;
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
            let instance = pre
                .instantiate_async(&mut store)
                .await
                .map_err(Error::Wasm)?;
            let guest = GuestIndices::new(&pre)
                .map_err(Error::Wasm)?
                .load(&mut store, &instance)
                .map_err(Error::Wasm)?;

            guest
                .call_initialize(&mut store, true, cfg.prelude.as_deref())
                .await
                .map_err(Error::Wasm)?;

            let data = wizer
                .snapshot_component(
                    &cx,
                    &mut WasmtimeWizerComponent {
                        store: &mut store,
                        instance,
                    },
                )
                .await
                .map_err(Error::Wasm)?;

            let component = Component::new(&engine, &data).map_err(Error::Wasm)?;
            component.serialize().map_err(Error::Wasm)
        })
    })
    .await
    .map_err(|e| Error::Other(e.into()))?
}

struct CompileHost;

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the host contract is asynchronous even when compilation rejects requests immediately"
)]
impl Host for CompileHost {
    async fn hostcall(
        &self,
        _call_type: &str,
        _payload: IsolaValue,
    ) -> core::result::Result<IsolaValue, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }

    async fn http_request_stream(
        &self,
        _req: HttpRequestStream,
    ) -> core::result::Result<HttpResponse, BoxError> {
        Err(std::io::Error::other("unsupported during compilation").into())
    }
}
