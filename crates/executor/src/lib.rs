#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::collapsible_if)]

pub mod env;
pub mod error;
mod resource;
mod trace_output;
mod vm;
mod vm_cache;
mod vm_manager;
mod wasm;

pub use env::Env;
pub use vm_manager::{ExecArgument, ExecArgumentValue, ExecSource, ExecStreamItem, VmManager};
