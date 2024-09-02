wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../apis/wit",
    async: true,
    trappable_imports: true,
    with: {
        "wasi:logging": crate::wasm::logging::bindings,
        "promptkit:vm": crate::wasm::vm::bindings,
    },
});
