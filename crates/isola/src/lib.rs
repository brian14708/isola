#![warn(missing_docs, rustdoc::all)]

//! Embed Isola WebAssembly sandboxes in a Rust application.
//!
//! Isola runs Python or JavaScript guest code from an Isola runtime bundle.
//! The crate supplies the embedding API; the language runtime itself is a
//! separate WebAssembly component. Host applications explicitly provide the
//! capabilities available to a guest through [`host::Host`], filesystem
//! mounts, and environment variables.
//!
//! # Lifecycle
//!
//! 1. Configure and compile a reusable [`sandbox::SandboxTemplate`].
//! 2. Instantiate an isolated [`sandbox::Sandbox`] with a host implementation
//!    and optional per-instance policy overrides.
//! 3. Load guest source with [`sandbox::Sandbox::eval_script`] or
//!    [`sandbox::Sandbox::eval_file`].
//! 4. Invoke guest functions with [`sandbox::Sandbox::call`] or stream their
//!    output through [`sandbox::Sandbox::call_with_sink`].
//!
//! Compiling a template is the expensive step. A template is immutable and can
//! be reused to create many sandboxes, while each sandbox keeps independent
//! guest state.
//!
//! The public API is grouped into three modules:
//!
//! - [`sandbox`] builds templates and manages guest execution.
//! - [`host`] defines hostcalls, HTTP forwarding, and output delivery.
//! - [`value`] converts the CBOR values exchanged at the host/guest boundary.
//!
//! # Quickstart
//!
//! This example uses the default `serde` feature. Download the standalone
//! Python runtime component first:
//!
//! ```bash
//! VERSION=v0.x.y
//! curl -L -o python.wasm.zst "https://github.com/brian14708/isola/releases/download/${VERSION}/python.wasm.zst"
//! ```
//!
//! ```no_run
//! # #[cfg(feature = "serde")]
//! use isola::{
//!     host::{Host, OutputTarget},
//!     sandbox::{SandboxOptions, SandboxTemplate, args},
//! };
//!
//! # #[cfg(feature = "serde")]
//! #[derive(Clone, Default)]
//! struct MyHost;
//!
//! # #[cfg(feature = "serde")]
//! impl Host for MyHost {}
//!
//! # #[cfg(feature = "serde")]
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let template = SandboxTemplate::builder()
//!         .cache(Some("./cache".into()))
//!         .max_memory(64 * 1024 * 1024)
//!         .build("./python.wasm.zst")
//!         .await?;
//!
//!     let mut sandbox = template
//!         .instantiate(MyHost, SandboxOptions::default())
//!         .await?;
//!
//!     sandbox
//!         .eval_script("def add(a, b):\n\treturn a + b", OutputTarget::discard())
//!         .await?;
//!
//!     let output = sandbox.call("add", args!(1_i64, 2_i64)?).await?;
//!     let result: i64 = output
//!         .result
//!         .ok_or_else(|| std::io::Error::other("missing result"))?
//!         .to_serde()?;
//!     assert_eq!(result, 3);
//!
//!     Ok(())
//! }
//! # #[cfg(not(feature = "serde"))]
//! # fn main() {}
//! ```
//!
//! # Cargo features
//!
//! - **`serde`** (enabled by default): adds serde and JSON conversion methods
//!   to [`value::Value`] and exports the `args!` macro.

/// Host integration traits and transport types.
pub mod host;
mod internal;
/// Runtime module and sandbox lifecycle APIs.
pub mod sandbox;
/// Opaque CBOR value helpers.
pub mod value;
