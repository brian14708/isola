use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
use tracing::event;
use wasmtime::component::HasData;
use wasmtime_wasi::p2::{IoImpl, WasiImpl, WasiView};

wasmtime::component::bindgen!({
    path: "../../apis/wit/deps/logging",
    trappable_imports: true,
});

pub use wasi::logging as bindings;

pub fn add_to_linker<T: WasiView>(
    linker: &mut wasmtime::component::Linker<T>,
) -> wasmtime::Result<()> {
    struct HasWasi<T>(T);
    impl<T: 'static> HasData for HasWasi<T> {
        type Data<'a> = WasiImpl<&'a mut T>;
    }
    bindings::logging::add_to_linker::<_, HasWasi<T>>(linker, |t| WasiImpl(IoImpl(t)))
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
