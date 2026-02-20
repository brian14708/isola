mod internal;

#[cfg(feature = "cbor")]
pub mod cbor;
pub mod error;
pub mod host;
pub mod module;
pub mod net;
#[cfg(feature = "request")]
pub mod request;
#[cfg(feature = "trace")]
pub mod trace;

pub const TRACE_TARGET_SCRIPT: &str = "isola::script";
#[cfg(feature = "trace")]
pub use trace::consts::TRACE_TARGET_OTEL;

pub use error::{Error, GuestErrorCode, Result};
pub use host::{
    BoxError, BoxedStream, Host, HttpBodyStream, HttpRequest, HttpResponse, OutputSink,
};
pub use module::{
    Arg, Args, CacheConfig, CallOptions, CompileConfig, InitConfig, Module, ModuleBuilder, Sandbox,
};
pub use net::{AclPolicy, AclPolicyBuilder, NetworkPolicy};
