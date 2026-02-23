# isola Python binding

Async-first Python bindings for the Isola runtime, built with `maturin` and PyO3.

## Python HTTP Host Handler

Outbound guest HTTP can be handled in Python per sandbox:

```python
import isola

async def handle_http(req: isola.HttpRequest) -> isola.HttpResponse:
    if req.url == "https://example.test/stream":
        async def body():
            yield b"chunk-1"
            yield b"chunk-2"

        return isola.HttpResponse(status=200, body=body())

    return isola.HttpResponse(
        status=200,
        headers={"content-type": "text/plain"},
        body=b"ok",
    )

sandbox.set_http_handler(handle_http)
```

If no handler is configured, guest `http_request` calls fail with `unsupported http_request`.
