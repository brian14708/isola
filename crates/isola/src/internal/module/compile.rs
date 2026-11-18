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
        module::{ModuleConfig, cache::write_cache_file_atomic},
        sandbox::{InstanceState, exports::GuestIndices},
    },
    sandbox::{Error, Result},
    value::Value as IsolaValue,
};

pub fn load_cached_component(engine: &Engine, path: &Path) -> wasmtime::Result<Component> {
    // Cache directories must be writable only by trusted processes.
    unsafe { Component::deserialize_file(engine, path) }
}

pub async fn load_or_compile_component(
    engine: &Engine,
    wasm_bytes: Vec<u8>,
    cfg: &ModuleConfig,
    cache_path: Option<&Path>,
) -> Result<Component> {
    let Some(cache_path) = cache_path else {
        return compile_snapshot(engine, cfg, wasm_bytes, Component::from_binary).await;
    };

    if let Ok(component) = load_cached_component(engine, cache_path) {
        return Ok(component);
    }

    let lock = cache_lock(cache_path);
    let _guard = lock.lock().await;

    // Another task may have populated the cache while this task waited for
    // the per-key lock.
    if let Ok(component) = load_cached_component(engine, cache_path) {
        return Ok(component);
    }

    let bytes = compile_snapshot(engine, cfg, wasm_bytes, Engine::precompile_component).await?;
    write_cache_file_atomic(cache_path, &bytes).await?;

    let component = load_cached_component(engine, cache_path).map_err(Error::Wasm)?;
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

async fn compile_snapshot<T: Send + 'static>(
    engine: &Engine,
    cfg: &ModuleConfig,
    wasm_bytes: Vec<u8>,
    compile: impl FnOnce(&Engine, &[u8]) -> wasmtime::Result<T> + Send + 'static,
) -> Result<T> {
    let engine = engine.clone();
    let cfg = cfg.clone();

    let span = tracing::Span::current();
    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    tokio::task::spawn_blocking(move || {
        let _subscriber = tracing::dispatcher::set_default(&dispatcher);
        let _span = span.enter();
        // Run initialization on a blocking worker so the compile-time guest
        // instantiation stays off the main async scheduler.
        tokio::runtime::Handle::current().block_on(async move {
            // Release the instrumented component, store, and initialization
            // buffers before compiling the snapshot into the final component.
            let data = {
                let wizer = Wizer::new();
                let (cx, instrumented_wasm) = wizer
                    .instrument_component(&wasm_bytes)
                    .map_err(Error::Wasm)?;

                let component = Component::new(&engine, &instrumented_wasm).map_err(Error::Wasm)?;
                drop(instrumented_wasm);
                let linker =
                    InstanceState::<CompileHost>::new_linker(&engine).map_err(Error::Wasm)?;
                let mut store = InstanceState::new(
                    &engine,
                    &cfg.directory_mappings,
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

                wizer
                    .snapshot_component(
                        &cx,
                        &mut WasmtimeWizerComponent {
                            store: &mut store,
                            instance,
                        },
                    )
                    .await
                    .map_err(Error::Wasm)?
            };
            drop(wasm_bytes);

            compile(&engine, &data).map_err(Error::Wasm)
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
