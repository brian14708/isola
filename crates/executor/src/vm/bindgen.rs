wasmtime::component::bindgen!({
    world: "sandbox",
    path: "../../apis/wit",
    async: true,

    with: {
        "promptkit:vm": crate::wasm::vm::bindings,
    },
});
