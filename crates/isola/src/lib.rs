//! Isola runtime embedding APIs.
//!
//! This crate exposes three public modules:
//! - [`sandbox`]: build/load guest templates and run sandboxes.
//! - [`host`]: host-side traits for hostcalls, HTTP, and output sinks.
//! - [`value`]: opaque CBOR values exchanged across the host/guest boundary.
//!
//! # Quickstart
//!
//! Download and extract the Python runtime bundle first:
//! ```bash
//! VERSION=v0.x.y
//! curl -L -o isola-python-runtime.tar.gz "https://github.com/brian14708/isola/releases/download/${VERSION}/isola-python-runtime.tar.gz"
//! tar xzf isola-python-runtime.tar.gz
//! mkdir -p isola-python-runtime/cache
//! ```
//! The archive extracts into `isola-python-runtime/` with `bin/python.wasm` and
//! `lib/`. Point `.build(...)` and `.mount(...)` at those extracted paths.
//!
//! ```no_run
//! use isola::{
//!     host::{Host, NoopOutputSink},
//!     sandbox::{DirPerms, FilePerms, SandboxOptions, SandboxTemplate, args},
//! };
//!
//! #[derive(Clone, Default)]
//! struct MyHost;
//!
//! #[async_trait::async_trait]
//! impl Host for MyHost {}
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let template = SandboxTemplate::<MyHost>::builder()
//!         .cache(Some("./isola-python-runtime/cache".into()))
//!         .max_memory(64 * 1024 * 1024)
//!         .mount(
//!             "./isola-python-runtime/lib",
//!             "/lib",
//!             DirPerms::READ,
//!             FilePerms::READ,
//!         )
//!         .build::<MyHost>("./isola-python-runtime/bin/python.wasm")
//!         .await?;
//!
//!     let mut sandbox = template
//!         .instantiate(MyHost, SandboxOptions::default())
//!         .await?;
//!
//!     sandbox
//!         .eval_script("def add(a, b):\n\treturn a + b", NoopOutputSink::shared())
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
//! ```

/// Host integration traits and transport types.
pub mod host;
mod internal;
/// Runtime module and sandbox lifecycle APIs.
pub mod sandbox;
/// Opaque CBOR value helpers.
pub mod value;
