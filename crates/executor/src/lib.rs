#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

pub mod env;
pub mod error;
mod resource;
mod trace_output;
pub mod vm;
mod vm_cache;
mod vm_manager;
mod wasm;

pub use env::Env;
pub use vm_manager::{
    ExecArgument, ExecArgumentValue, ExecSource, ExecStreamItem, MpscOutputCallback, VmManager,
};
