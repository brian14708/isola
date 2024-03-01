# PromptKit

## Build from source

Ensure you have the following tools installed:
- [cargo-make](https://github.com/sagiegurari/cargo-make)
- [wasm-tools](https://github.com/bytecodealliance/wasm-tools)
- [wizer](https://github.com/bytecodealliance/wizer)
- [binaryen](https://github.com/WebAssembly/binaryen)

Execute the following commands to build and launch PromptKit:

```
cd wasm && cargo make
cargo run -p promptkit_server
```