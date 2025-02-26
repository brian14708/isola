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
                name: "promptkit.log",
                target: "promptkit::log",
                tracing::Level::TRACE,
                promptkit.log.output = &message,
                promptkit.log.context = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Debug => event!(
                name: "promptkit.log",
                target: "promptkit::log",
                tracing::Level::DEBUG,
                promptkit.log.output = &message,
                promptkit.log.context = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Info => event!(
                name: "promptkit.log",
                target: "promptkit::log",
                tracing::Level::INFO,
                promptkit.log.output = &message,
                promptkit.log.context = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Warn => event!(
                name: "promptkit.log",
                target: "promptkit::log",
                tracing::Level::WARN,
                promptkit.log.output = &message,
                promptkit.log.context = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Error => event!(
                name: "promptkit.log",
                target: "promptkit::log",
                tracing::Level::ERROR,
                promptkit.log.output = &message,
                promptkit.log.context = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Critical => event!(
                name: "promptkit.log",
                target: "promptkit::log",
                tracing::Level::ERROR,
                promptkit.log.output = &message,
                promptkit.log.context = &context,
                promptkit.user = true,
            ),
        }
        Ok(())
    }
}
