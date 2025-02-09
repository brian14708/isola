use tracing::event;
use wasmtime_wasi::WasiView;

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
        F: Fn(&mut T) -> &mut dyn WasiView,
    {
        val
    }
    let closure = type_annotate::<T, _>(|t| t);
    bindings::logging::add_to_linker_get_host(linker, closure)
}

impl bindings::logging::Host for dyn WasiView + '_ {
    fn log(
        &mut self,
        log_level: bindings::logging::Level,
        context: String,
        message: String,
    ) -> wasmtime::Result<()> {
        match log_level {
            bindings::logging::Level::Trace => event!(
                target: "promptkit::log",
                tracing::Level::TRACE,
                promptkit.log.output = &message,
                promptkit.log.group = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Debug => event!(
                target: "promptkit::log",
                tracing::Level::DEBUG,
                promptkit.log.output = &message,
                promptkit.log.group = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Info => event!(
                target: "promptkit::log",
                tracing::Level::INFO,
                promptkit.log.output = &message,
                promptkit.log.group = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Warn => event!(
                target: "promptkit::log",
                tracing::Level::WARN,
                promptkit.log.output = &message,
                promptkit.log.group = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Error => event!(
                target: "promptkit::log",
                tracing::Level::ERROR,
                promptkit.log.output = &message,
                promptkit.log.group = &context,
                promptkit.user = true,
            ),
            bindings::logging::Level::Critical => event!(
                target: "promptkit::log",
                tracing::Level::ERROR,
                promptkit.log.output = &message,
                promptkit.log.group = &context,
                promptkit.user = true,
            ),
        }
        Ok(())
    }
}
