# Python Guest API

The `sandbox` package is available to code running inside a Python guest
runtime.

This page documents guest-side APIs. For the host-side embedding SDK that
builds templates and starts sandboxes, see [Python Host API](python-api.md).

## Execution Model

Guest entrypoints can be regular functions or `async def` coroutines:

```python
from sandbox.asyncio import hostcall


def add(a, b):
    return a + b


async def lookup_user(user_id):
    return await hostcall("lookup_user", {"user_id": user_id})
```

If a guest function returns an iterable or async iterable, each yielded item is
emitted to the host as a partial result and the final result is `None`:

```python
def stream_values():
    for i in range(3):
        yield i
```

Values crossing the host/guest boundary should be JSON-like unless you are
working with raw HTTP bodies.

## `sandbox.asyncio`

Import async helpers with:

```python
from sandbox.asyncio import hostcall, run, subscribe
```

### `await hostcall(call_type, payload) -> object`

Calls a host-registered callback and resolves to the returned value.

```python
from sandbox.asyncio import hostcall


async def main(user_id):
    return await hostcall("lookup_user", {"user_id": user_id})
```

### `run(main)`

Runs a coroutine or async generator on the guest poll loop from synchronous
guest code.

Use this when you need to drive async work yourself from a synchronous helper or
module initialization path. When the host directly invokes an `async def`
function, Isola awaits it automatically and you do not need `run(...)`.

```python
from sandbox.asyncio import hostcall, run


async def fetch_user(user_id):
    return await hostcall("lookup_user", {"user_id": user_id})


def main(user_id):
    return run(fetch_user(user_id))
```

### `await subscribe(pollable) -> object`

Low-level helper for awaiting native guest pollables. Most guest code should use
`hostcall(...)` or `sandbox.http.fetch(...)` instead of calling `subscribe(...)`
directly.

## `sandbox.http`

Import the guest HTTP client with:

```python
from sandbox.http import fetch
```

Guest HTTP is only available when the host enables outbound requests with
`http_handler=` or `httpHandler=`. See [Python Host API](python-api.md) and
[Node.js Host API](nodejs-api.md).

### `fetch(...) -> Request`

```python
fetch(
    method,
    url,
    *,
    params=None,
    headers=None,
    files=None,
    body=None,
    timeout=None,
    proxy=None,
)
```

Supported request features:

- `params`: query-string mapping
- `headers`: string header mapping
- `body`: raw `bytes` or a JSON-serializable object
- `files`: multipart form fields; each value may be raw bytes, a file object, or
  `(filename, fileobj, content_type)`
- `timeout`: first-byte timeout in seconds
- `proxy`: sets the `x-isola-proxy` header for host policies that honor it

If `body` is an object and `content-type` is not already set, the runtime uses
`application/json`.

### Synchronous usage

```python
from sandbox.http import fetch


def main(url):
    with fetch("GET", url, params={"q": "hello"}) as resp:
        return {
            "status": resp.status,
            "headers": resp.headers,
            "body": resp.text(),
        }
```

`Response` exposes:

- `status`
- `headers`
- `read(size=-1) -> bytes`
- `text() -> str`
- `json() -> object`
- `iter_bytes()`
- `iter_lines()`
- `iter_sse()`
- `close()`

### Asynchronous usage

```python
from sandbox.http import fetch


async def main(url):
    async with fetch("GET", url) as resp:
        return await resp.atext()
```

`AsyncResponse` exposes:

- `status`
- `headers`
- `aread(size=-1) -> bytes`
- `atext() -> str`
- `ajson() -> object`
- `aiter_bytes()`
- `aiter_lines()`
- `aiter_sse()`
- `close()`

### Server-sent events

`iter_sse()` and `aiter_sse()` yield `ServerSentEvent` values with:

- `id`
- `event`
- `data`

## `sandbox.importlib`

Import remote modules over HTTP with:

```python
from sandbox.importlib import http
```

`http(url)` returns a context manager that temporarily adds an importer to
`sys.meta_path`:

```python
from sandbox.importlib import http


def main():
    with http("https://example.com/modules"):
        import helpers

    return helpers.answer()
```

The URL may point at a module tree or a zip archive. This importer is also used
internally for Isola's URL-based dependency loading.

## `sandbox.logging`

Import structured log helpers with:

```python
from sandbox.logging import debug, error, info, warning
```

These emit guest log events back to the host sink. `print(...)` still writes to
stdout.

## `sandbox.serde`

Import serialization helpers with:

```python
from sandbox.serde import dumps, loads
```

Supported formats are `"json"`, `"yaml"`, and `"cbor"`:

```python
from sandbox.serde import dumps, loads


payload = dumps({"hello": "world"}, "json")
value = loads(payload, "json")
```

`dumps(value, format)` returns a `str` for JSON/YAML and `bytes` for CBOR.
`loads(value, format)` performs the reverse conversion.
