// based on:
// - https://blog.yoshuawuyts.com/building-an-async-runtime-for-wasi/
// - https://github.com/yoshuawuyts/wasm-http-tools/tree/main/crates/wasi-async-runtime

mod block_on;
mod polling;
mod reactor;

pub use block_on::block_on;
pub use reactor::Reactor;
