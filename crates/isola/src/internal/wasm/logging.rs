use crate::TRACE_TARGET_SCRIPT;
use tracing::event;
use wasmtime::component::HasData;

wasmtime::component::bindgen!({
    path: "../../specs/wit/deps/logging",
});

pub use wasi::logging as bindings;

struct HasWasi<T>(T);
impl<T: 'static> HasData for HasWasi<T> {
    type Data<'a> = LoggingImpl;
}

pub fn add_to_linker<T>(linker: &mut wasmtime::component::Linker<T>) -> wasmtime::Result<()> {
    bindings::logging::add_to_linker::<T, HasWasi<T>>(linker, |_t| LoggingImpl)
}

struct LoggingImpl;

impl bindings::logging::Host for LoggingImpl {
    fn log(&mut self, log_level: bindings::logging::Level, context: String, message: String) {
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
            bindings::logging::Level::Error | bindings::logging::Level::Critical => event!(
                name: "log",
                target: TRACE_TARGET_SCRIPT,
                tracing::Level::ERROR,
                log.output = &message,
                log.context = &context,
            ),
        }
    }
}
