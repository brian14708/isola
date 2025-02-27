use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tracing::event;
use wasmtime_wasi::{WasiImpl, WasiView};

wasmtime::component::bindgen!({
    path: "../../apis/wit/deps/logging",
    trappable_imports: true,
});

pub use wasi::logging as bindings;

pub fn add_to_linker<T: WasiView>(
    linker: &mut wasmtime::component::Linker<T>,
) -> wasmtime::Result<()> {
    fn type_annotate<T, F>(val: F) -> F
    where
        F: Fn(&mut T) -> WasiImpl<&mut T>,
    {
        val
    }
    let closure = type_annotate::<T, _>(|t| WasiImpl(wasmtime_wasi::IoImpl(t)));
    bindings::logging::add_to_linker_get_host(linker, closure)
}

impl<T: WasiView> bindings::logging::Host for WasiImpl<T> {
    fn log(
        &mut self,
        log_level: bindings::logging::Level,
        context: String,
        message: String,
    ) -> wasmtime::Result<()> {
        match log_level {
            bindings::logging::Level::Trace => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::TRACE,
                log.output = &message,
                log.context = &context,
            ),
            bindings::logging::Level::Debug => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::DEBUG,
                log.output = &message,
                log.context = &context,
            ),
            bindings::logging::Level::Info => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::INFO,
                log.output = &message,
                log.context = &context,
            ),
            bindings::logging::Level::Warn => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::WARN,
                log.output = &message,
                log.context = &context,
            ),
            bindings::logging::Level::Error => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::ERROR,
                log.output = &message,
                log.context = &context,
            ),
            bindings::logging::Level::Critical => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::ERROR,
                log.output = &message,
                log.context = &context,
            ),
        }
        Ok(())
    }
}
