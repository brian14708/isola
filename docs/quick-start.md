# Quick Start

This page covers the fastest ways to get Isola running:

- Embed the runtime directly from Python with the `isola` SDK
- Embed the runtime directly from Node.js with `isola-core`

## Python SDK

Install the SDK:

```bash
pip install isola
```

Then compile a Python sandbox template and run code inside it:

```python
import asyncio

from isola import build_template


async def main() -> None:
    template = await build_template(
        "python",
        max_memory=64 * 1024 * 1024,
    )

    async with template.create() as sandbox:
        await sandbox.load_script(
            "def add(a, b):\n"
            "    return a + b\n"
        )
        result = await sandbox.run("add", 1, 2)
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

from isola import build_template


async def main() -> None:
    async def lookup_user(payload: dict[str, object]) -> object:
        user_id = int(payload["user_id"])
        return {"user_id": user_id, "name": f"user-{user_id}"}

    template = await build_template(
        "python",
        max_memory=64 * 1024 * 1024,
    )

    async with template.create(hostcalls={"lookup_user": lookup_user}) as sandbox:
        await sandbox.load_script(
            "from sandbox.asyncio import hostcall\n"
            "\n"
            "async def lookup_user(user_id):\n"
            "    return await hostcall('lookup_user', {'user_id': user_id})\n"
        )
        result = await sandbox.run("lookup_user", 7)
        print(result)


asyncio.run(main())
```

Expected output:

```text
{'user_id': 7, 'name': 'user-7'}
```

If you omit `runtime_path`, the SDK downloads the matching runtime bundle on first use, verifies it, and caches it under `~/.cache/isola/runtimes/`. To use a runtime you unpacked yourself, pass `runtime_path` and `runtime_lib_dir` to `build_template(...)` instead. Outbound guest HTTP is disabled by default; pass `http_handler=True` for the built-in `httpx` bridge or supply `http_handler=` with your own async handler. Use `SandboxContext` only when you want explicit context ownership.

For the SDK surface area and type reference, see [Python API](python-api.md).

## JavaScript / TypeScript SDK

Install the SDK:

```bash
npm install isola-core
```

Compile a sandbox template and run code inside it:

```typescript
import { buildTemplate } from "isola-core";

const template = await buildTemplate("python", {
  maxMemory: 64 * 1024 * 1024,
});

await using sandbox = await template.create();
await sandbox.start();
await sandbox.loadScript("def add(a, b):\n    return a + b\n");
const result = await sandbox.run("add", [1, 2]);
console.log(result); // 3
```

Expected output:

```text
3
```

To call back into the host from guest code, pass a `hostcalls` map when creating the sandbox:

```typescript
import { buildTemplate } from "isola-core";

const template = await buildTemplate("python");

await using sandbox = await template.create({
  hostcalls: {
    lookup_user: async (payload) => {
      const { user_id } = payload as { user_id: number };
      return { user_id, name: `user-${user_id}` };
    },
  },
});

await sandbox.start();
await sandbox.loadScript(
  "from sandbox.asyncio import hostcall\n" +
    "\n" +
    "async def lookup_user(user_id):\n" +
    "    return await hostcall('lookup_user', {'user_id': user_id})\n",
);
const result = await sandbox.run("lookup_user", [7]);
console.log(result); // { user_id: 7, name: 'user-7' }
```

If you omit `runtimePath`, the SDK downloads the matching runtime bundle on first use, verifies it, and caches it under `~/.cache/isola/runtimes/`. To use a runtime you unpacked yourself, pass `runtimePath` (and `runtimeLibDir` for Python) to `buildTemplate(...)` instead. Use `SandboxContext` only when you want explicit context ownership.

For the SDK surface area and type reference, see [Node.js API](nodejs-api.md).
