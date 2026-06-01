#![allow(linker_messages)]

mod error;
mod script;
mod serde;
mod transpile;
mod wasm;

#[expect(unused)]
use wasm::Global;
