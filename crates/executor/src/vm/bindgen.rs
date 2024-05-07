wasmtime::component::bindgen!({
    world: "sandbox",
    async: true,

    with: {
        "promptkit:script/http-client/request": super::http_client::Request,
        "promptkit:script/http-client/response": super::http_client::Response,
        "promptkit:script/http-client/response-sse-body": super::http_client::ResponseSseBody,
        "promptkit:script/host-api/argument-iterator": super::host_types::ArgumentIterator,

        "promptkit:script/llm/tokenizer": super::llm::Tokenizer,
    },
});

pub use promptkit::script::{host_api, http_client};
