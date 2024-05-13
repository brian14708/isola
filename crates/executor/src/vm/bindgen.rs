wasmtime::component::bindgen!({
    world: "sandbox",
    async: true,

    with: {
        "wasi": wasmtime_wasi::bindings,
        "promptkit:http/client": super::http,
        "promptkit:llm/tokenizer/tokenizer": super::llm::Tokenizer,
        "promptkit:script/host-api/argument-iterator": super::host_types::ArgumentIterator,
    },
});

pub use promptkit::script::host_api;
