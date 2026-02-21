use std::future::Future;

use wasmtime::component::{HasData, Linker};

wasmtime::component::bindgen!({
    path: "wit/deps/logging",
    imports: {
        default: async | trappable,
    },
    exports: {
        default: async | trappable,
    },
    ownership: Borrowing {
        duplicate_if_necessary: true
    },
});

pub use wasi::logging as bindings;

pub trait HostView: Send {
    fn emit_log(
        &mut self,
        log_level: bindings::logging::Level,
        context: &str,
        message: &str,
    ) -> impl Future<Output = wasmtime::Result<()>> + Send;
}

impl<T: ?Sized + HostView> HostView for &mut T {
    async fn emit_log(
        &mut self,
        log_level: bindings::logging::Level,
        context: &str,
        message: &str,
    ) -> wasmtime::Result<()> {
        T::emit_log(self, log_level, context, message).await
    }
}

struct HasWasi<T>(T);
impl<T: 'static + HostView> HasData for HasWasi<T> {
    type Data<'a> = LoggingImpl<&'a mut T>;
}

pub fn add_to_linker<T: HostView>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    bindings::logging::add_to_linker::<T, HasWasi<T>>(linker, |t| LoggingImpl(t))
}

struct LoggingImpl<T: Send>(T);

impl<T: HostView> bindings::logging::Host for LoggingImpl<T> {
    async fn log(
        &mut self,
        log_level: bindings::logging::Level,
        context: String,
        message: String,
    ) -> wasmtime::Result<()> {
        self.0.emit_log(log_level, &context, &message).await
    }
}
