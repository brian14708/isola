// Public API modules
pub mod environment;
pub mod error;
pub mod runtime;

// Internal implementation - not part of public API
mod internal;

// Re-export main public types for convenience
pub use environment::{BoxedStream, Environment, WebsocketMessage};
pub use error::{Error, ErrorCode, Result};
pub use runtime::{Argument, Instance, Runtime, RuntimeBuilder};
