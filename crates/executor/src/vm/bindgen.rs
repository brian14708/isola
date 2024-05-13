wasmtime::component::bindgen!({
    world: "sandbox",
    async: true,

    with: {
        "promptkit:vm": crate::wasm::vm::bindings,
    },
});
