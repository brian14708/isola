#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]

mod atomic_cell;
mod env;
pub mod error;
mod resource;
pub mod trace;
mod trace_output;
mod vm;
mod vm_cache;
mod vm_manager;

pub use env::Env;
pub use vm_manager::{ExecArgument, ExecSource, ExecStreamItem, VmManager};
