# Quick Start

This page covers the fastest ways to get Isola running:

- Run the packaged HTTP server with Docker and call it with `curl`
- Embed the runtime directly from Python with the `isola` SDK

## Docker and `curl`

Start the published server image:

```bash
docker run --rm -p 3000:3000 ghcr.io/brian14708/isola:latest
```

Execute a small Python generator over HTTP as a stream:

```bash
curl -N http://127.0.0.1:3000/v1/execute/stream \
  -H 'content-type: application/json' \
  -d '{
    "runtime": "python",
    "script": "def count(n):\n    for i in range(n):\n        yield i",
    "function": "count",
    "args": [3]
  }'
```

Expected response:

```text
event: data
data: {"value":0}

event: data
data: {"value":1}

event: data
data: {"value":2}

event: done
data: {}
```

The same server also exposes an OpenAPI document at `http://127.0.0.1:3000/openapi.json`.

If you set `"trace": true` in the request body, trace records are emitted as separate `event: trace` entries before the final `event: done`.

## Python SDK

Install the SDK:

```bash
pip install isola
```

Then compile a Python sandbox template and run code inside it:

```python
import asyncio

from isola import SandboxManager


async def main() -> None:
    async with SandboxManager() as manager:
        template = await manager.compile_template(
            "python",
            max_memory=64 * 1024 * 1024,
        )

        sandbox = await template.create()
        async with sandbox:
            await sandbox.load_script(
                "def add(a, b):\n"
                "    return a + b\n"
            )
            result = await sandbox.run("add", [1, 2])
            print(result)


asyncio.run(main())
```

Expected output:

```text
3
```

To call back into the host from Python guest code, pass a `hostcalls` mapping when you create the sandbox, then call it from the guest with `sandbox.asyncio.hostcall(...)`:

```python
import asyncio

from isola import SandboxManager


async def main() -> None:
    async def lookup_user(payload: dict[str, object]) -> object:
        user_id = int(payload["user_id"])
        return {"user_id": user_id, "name": f"user-{user_id}"}

    async with SandboxManager() as manager:
        template = await manager.compile_template(
            "python",
            max_memory=64 * 1024 * 1024,
        )

        sandbox = await template.create(hostcalls={"lookup_user": lookup_user})
        async with sandbox:
            await sandbox.load_script(
                "from sandbox.asyncio import hostcall\n"
                "\n"
                "async def lookup_user(user_id):\n"
                "    return await hostcall('lookup_user', {'user_id': user_id})\n"
            )
            result = await sandbox.run("lookup_user", [7])
            print(result)


asyncio.run(main())
```

Expected output:

```text
{'user_id': 7, 'name': 'user-7'}
```

If you omit `runtime_path`, the SDK downloads the matching runtime bundle on first use, verifies it, and caches it under `~/.cache/isola/runtimes/`. To use a runtime you unpacked yourself, pass `runtime_path` and `runtime_lib_dir` to `compile_template(...)` instead.

For the SDK surface area and type reference, see [Python API](python-api.md).
