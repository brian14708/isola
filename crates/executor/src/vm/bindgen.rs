wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../apis/wit",
    async: true,
    trappable_imports: true,
    with: {
        "promptkit:vm": crate::wasm::vm::bindings,
    },
});
