# Node.js API

The `isola-core` package exposes an async API for compiling sandbox templates and
running code inside isolated runtimes from Node.js.

## Install

```bash
npm install isola-core
```

## Runtime Resolution

When `runtimePath` is omitted from `buildTemplate(...)`, the SDK
resolves the runtime automatically, downloads the matching release asset on
first use, verifies its SHA-256 digest, and caches it under
`~/.cache/isola/runtimes/`.

Supported runtime names:

- `"python"`
- `"js"`

Use `version` to resolve a specific release tag. To use a runtime you unpacked
yourself, pass `runtimePath` and, for Python runtimes, `runtimeLibDir`.

## Lifecycle

The normal flow is:

1. `await buildTemplate(...)`
2. `await template.create(...)`
3. `await sandbox.start()`
4. `await sandbox.loadScript(...)`
5. `await sandbox.run(...)` or iterate `sandbox.runStream(...)`
6. `sandbox.close()`

```typescript
import { buildTemplate } from "isola-core";

const template = await buildTemplate("python");
const sandbox = await template.create();

try {
  await sandbox.start();
  await sandbox.loadScript("def hello(name):\n    return f'hello {name}'");
  const result = await sandbox.run("hello", ["world"]);
  console.log(result);
} finally {
  sandbox.close();
}
```

## Core Types

### `buildTemplate(...)`

Builds and returns a reusable sandbox template using an internal `SandboxContext`.

```typescript
import { buildTemplate } from "isola-core";

const template = await buildTemplate(runtime, options);
```

`buildTemplate(...)` accepts:

- `runtime`: `"python"` or `"js"`
- `runtimePath`: directory or path used to initialize the runtime bundle
- `version`: optional release tag to resolve when auto-downloading a runtime
- `cacheDir`: template cache directory, or `null` to disable caching
- `maxMemory`: template memory limit in bytes
- `prelude`: code injected before user scripts
- `runtimeLibDir`: runtime library directory for manually provided Python runtimes
- `mounts`: `MountConfig[]`
- `env`: `Record<string, string>`

### `SandboxContext`

Advanced API for explicitly owning a template compilation context. It exposes
the same template-building behavior as the top-level helper, plus explicit
`close()` ownership when you need to manage the context directly.

### `SandboxTemplate`

Instantiates sandboxes from a compiled template.

```typescript
const sandbox = await template.create(options);
```

`create(...)` accepts:

- `maxMemory`: per-sandbox memory limit in bytes
- `mounts`: `MountConfig[]`
- `env`: `Record<string, string>`
- `hostcalls`: `Record<string, (payload: JsonValue) => Promise<unknown>>`
- `httpHandler`: `(req: HttpRequest) => Promise<HttpResponse>`

### `Sandbox`

Runs guest code inside an instantiated sandbox.

```typescript
await sandbox.start();
await sandbox.loadScript(code);
const result = await sandbox.run(name, args);
```

Public methods:

- `await start()`
- `await loadScript(code)`
- `await run(name, args?, kwargs?) -> JsonValue | null`
- `runStream(name, args?, kwargs?) -> AsyncGenerator<Event>`
- `close()`
- `await sandbox[Symbol.asyncDispose]()`

Call `start()` before loading scripts or executing functions.

## Arguments

`run(...)` and `runStream(...)` accept positional JSON-like values directly:

```typescript
const result = await sandbox.run("add", [1, 2]);
```

Pass kwargs as the third argument:

```typescript
const result = await sandbox.run("greet", [], {
  name: "World",
  greeting: "Hi",
});
```

Use `Arg` to pass a named argument:

```typescript
import { Arg } from "isola-core";

const result = await sandbox.run("greet", [
  new Arg("World", "name"),
  new Arg("Hi", "greeting"),
]);
```

The second argument must always be the positional args array. If you need to
pass a single object as a positional argument, wrap it in that array:
`sandbox.run("echo", [{ hello: "world" }])`.

## Hostcalls

Register host callbacks when the sandbox is created. Each handler receives the
decoded JSON payload for its call name and must return a JSON-serializable
value.

```typescript
import { buildTemplate } from "isola-core";

const template = await buildTemplate("python");
const sandbox = await template.create({
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
```

## Events and Results

`run(...)` resolves to the final return value directly:

```typescript
const result = await sandbox.run("add", [1, 2]);
```

Use `runStream(...)` when you need yielded values or process output:

```typescript
for await (const event of sandbox.runStream("compute")) {
  switch (event.type) {
    case "result":
      console.log("intermediate:", event.data);
      break;
    case "end":
      console.log("final:", event.data);
      break;
    case "stdout":
      console.log("stdout:", event.data);
      break;
    case "stderr":
      console.error("stderr:", event.data);
      break;
    case "error":
      console.error("error:", event.data);
      break;
    case "log":
      console.log("log:", event.data);
      break;
  }
}
```

`Event` is a union of:

- `{ type: "result"; data: JsonValue }`
- `{ type: "end"; data: JsonValue | null }`
- `{ type: "stdout"; data: string }`
- `{ type: "stderr"; data: string }`
- `{ type: "error"; data: string }`
- `{ type: "log"; data: string }`

## Filesystem and Environment

Use `MountConfig` to mount host paths into the guest:

```typescript
const mount = {
  host: "./data",
  guest: "/workspace",
  dir_perms: "read",
  file_perms: "read",
};
```

`dir_perms` and `file_perms` accept:

- `"read"`
- `"write"`
- `"read-write"`

Environment variables can be supplied in both template and sandbox config via
`env: { KEY: "value" }`.

## HTTP Bridge

When guest code makes outbound HTTP requests, the sandbox calls the configured
`httpHandler`.

```typescript
import type { HttpRequest, HttpResponse } from "isola-core";

async function httpHandler(request: HttpRequest): Promise<HttpResponse> {
  return {
    status: 200,
    headers: { "content-type": "text/plain" },
    body: Buffer.from("hello world"),
  };
}
```

Request and response shapes:

- `HttpRequest = { method, url, headers, body }`
- `HttpResponse = { status, headers?, body? }`

`HttpRequest.body` is `Buffer | null`.

`HttpResponse.body` may be:

- `Buffer`
- `null`
- omitted

## Errors

`buildTemplate(...)`, `start()`, `loadScript(...)`, and `run(...)` reject when
runtime setup or execution fails.

`runStream(...)` yields `{ type: "error", data: string }` events for sandbox
execution failures, and may still throw for setup or transport failures that do
not surface as stream events.
