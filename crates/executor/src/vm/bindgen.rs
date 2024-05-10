wasmtime::component::bindgen!({
    world: "sandbox",
    async: true,

    with: {
        "wasi": wasmtime_wasi::bindings,
        "promptkit:http/client": super::http,
        "promptkit:script/host-api/argument-iterator": super::host_types::ArgumentIterator,
        "promptkit:script/llm/tokenizer": super::llm::Tokenizer,
    },
});

pub use promptkit::script::host_api;
