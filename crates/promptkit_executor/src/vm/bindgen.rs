wasmtime::component::bindgen!({
    world: "python-vm",
    async: true,

    with: {
        "promptkit:python/http-client/request": super::http_client::Request,
        "promptkit:python/http-client/response": super::http_client::Response,
        "promptkit:python/http-client/response-sse-body": super::http_client::ResponseSseBody,
    },
});

pub use promptkit::python::http_client;
