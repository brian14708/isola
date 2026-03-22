# Python API

The `isola` package exposes an async API for compiling sandbox templates and running code inside isolated runtimes.

## Install

```bash
pip install isola
```

## Runtime Resolution

Use `resolve_runtime(runtime, *, version=None)` to fetch the runtime bundle config directly:

```python
from isola import resolve_runtime

config = await resolve_runtime("python")
```

When `runtime_path` is omitted from `SandboxManager.compile_template(...)`, the SDK resolves the runtime automatically, downloads the matching release asset on first use, verifies its digest, and caches it under `~/.cache/isola/runtimes/`.

Supported runtime names:

- `"python"`
- `"js"`

## Lifecycle

The normal flow is:

1. Create a `SandboxManager`
2. `await manager.compile_template(...)`
3. `await template.create(...)`
4. `async with sandbox: ...`
5. `await sandbox.load_script(...)`
6. `await sandbox.run(...)` or iterate `sandbox.run_stream(...)`

```python
import asyncio

from isola import SandboxManager


async def main() -> None:
    async with SandboxManager() as manager:
        template = await manager.compile_template("python")
        sandbox = await template.create()
        async with sandbox:
            await sandbox.load_script("def hello(name):\n    return f'hello {name}'")
            result = await sandbox.run("hello", ["world"])
            print(result.final)


asyncio.run(main())
```

## Core Types

### `SandboxManager`

Creates reusable sandbox templates.

```python
manager = SandboxManager()
template = await manager.compile_template(runtime, *, version=None, **template_config)
manager.close()
```

`compile_template(...)` accepts:

- `runtime`: `"python"` or `"js"`
- `version`: optional release tag to resolve when auto-downloading a runtime
- `runtime_path`: directory or path used to initialize the runtime bundle
- `runtime_lib_dir`: runtime library directory, required for Python runtimes that are provided manually
- `cache_dir`: template cache directory
- `max_memory`: template memory limit in bytes
- `prelude`: code injected before user scripts
- `mounts`: `list[MountConfig]`
- `env`: `dict[str, str]`

`SandboxManager` supports both sync and async context-manager cleanup.

### `SandboxTemplate`

Instantiates sandboxes from a compiled template.

```python
sandbox = await template.create(**sandbox_config)
```

`create(...)` accepts:

- `max_memory`: per-sandbox memory limit in bytes
- `mounts`: `list[MountConfig]`
- `env`: `dict[str, str]`
- `http_handler`: async callable used for outbound guest HTTP requests

### `Sandbox`

Runs guest code inside an instantiated sandbox.

```python
async with sandbox:
    await sandbox.load_script(code)
    result = await sandbox.run(name, args=None)
```

Public methods:

- `set_callback(callback)`: receives `Event` values during execution
- `set_http_handler(handler)`: overrides the HTTP bridge for the sandbox
- `await load_script(code)`
- `await run(name, args=None) -> RunResult`
- `run_stream(name, args=None) -> AsyncIterator[Event]`
- `close()`
- `await aclose()`

Use `async with sandbox:` before executing code. Entering the async context starts the sandbox.

## Arguments and Streaming

`run(...)` and `run_stream(...)` accept positional JSON values directly:

```python
result = await sandbox.run("add", [1, 2])
```

Use `Arg(value, name="...")` to pass a named argument:

```python
from isola import Arg

result = await sandbox.run("add", [Arg(2, name="b"), Arg(1, name="a")])
```

Use `StreamArg` for JSON streams passed into guest code:

```python
from isola import StreamArg

stream = StreamArg.from_iterable([1, 2, 3])
result = await sandbox.run("consume", [stream])
```

Available constructors:

- `StreamArg.from_iterable(values, *, name=None, capacity=1024)`
- `StreamArg.from_async_iterable(values, *, name=None, capacity=1024)`

## Events and Results

`run_stream(...)` yields `Event` objects:

```python
async for event in sandbox.run_stream("emit"):
    print(event.kind, event.data)
```

`Event.kind` is one of:

- `"result"`
- `"end"`
- `"stdout"`
- `"stderr"`
- `"error"`
- `"log"`

`run(...)` collects those events into `RunResult`:

- `results`: streamed JSON values
- `final`: final JSON return value
- `stdout`: captured stdout lines
- `stderr`: captured stderr lines
- `logs`: runtime log messages
- `errors`: execution errors emitted by the runtime

## Filesystem and Environment

Use `MountConfig` to mount host paths into the guest:

```python
from isola import MountConfig

mount = MountConfig(
    host="./data",
    guest="/workspace",
    dir_perms="read",
    file_perms="read",
)
```

`dir_perms` and `file_perms` accept:

- `"read"`
- `"write"`
- `"read-write"`

Environment variables can be supplied in both template and sandbox config via `env={"KEY": "value"}`.

## HTTP Bridge

When guest code makes outbound HTTP requests, the sandbox uses an async Python handler that returns `HttpResponse`.

```python
from collections.abc import AsyncIterator

from isola import HttpRequest, HttpResponse


async def body() -> AsyncIterator[bytes]:
    yield b"hello "
    yield b"world"


async def http_handler(request: HttpRequest) -> HttpResponse:
    return HttpResponse(
        status=200,
        headers={"content-type": "text/plain"},
        body=body(),
    )
```

Request and response models:

- `HttpRequest(method, url, headers, body)`
- `HttpResponse(status, headers=None, body=None)`

`HttpResponse.body` may be:

- `bytes`
- `AsyncIterable[bytes]`
- `None`

## Errors

The package exports these exception types:

- `IsolaError`
- `InvalidArgumentError`
- `InternalError`
- `StreamFullError`
- `StreamClosedError`
