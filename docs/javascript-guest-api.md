# JavaScript Guest API

JavaScript guests run inside an Isola-provided runtime with a small set of
injected globals.

This page documents those guest-side globals. For the host-side embedding SDK
that builds templates and starts sandboxes, see
[Node.js Host API](nodejs-api.md).

## Execution Model

Guest entrypoints can be plain functions or `async function`s:

```javascript
function add(a, b) {
  return a + b;
}

async function lookupUser(userId) {
  return await hostcall("lookup_user", { user_id: userId });
}
```

Generators and async generators stream partial results to the host:

```javascript
async function* streamValues() {
  yield 1;
  yield 2;
  yield 3;
}
```

For generators, yielded values become partial results and a final returned value
becomes the end result if it is not `undefined`.

The sandbox is not a full browser or Node.js environment. Only the globals
documented below are part of the supported guest runtime surface.

## `hostcall(callType, payload)`

`hostcall(...)` is a top-level async helper that calls a host-registered
callback and resolves to the returned value.

```javascript
async function main(userId) {
  return await hostcall("lookup_user", { user_id: userId });
}
```

Payloads and results should be JSON-like values.

## HTTP Globals

Guest HTTP is only available when the host enables outbound requests with
`http_handler=` or `httpHandler=`. See [Python Host API](python-api.md) and
[Node.js Host API](nodejs-api.md).

The runtime injects these globals:

- `fetch`
- `Headers`
- `Request`
- `Response`
- `AbortController`
- `AbortSignal`
- `URL`
- `URLSearchParams`

### `fetch(input, init?)`

```javascript
async function main(url) {
  const resp = await fetch(url + "/hello", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: { hello: "world" },
  });

  return await resp.json();
}
```

Supported request body types:

- `string`
- `ArrayBuffer`
- `TypedArray`
- `URLSearchParams`
- plain JSON-like objects

If the request body is a plain object and `content-type` is not already set, the
runtime uses `application/json`. `URLSearchParams` bodies use
`application/x-www-form-urlencoded;charset=UTF-8`.

`GET` and `HEAD` requests cannot have a body.

### `Headers`

`Headers` supports construction from another `Headers`, an iterable of header
pairs, or an object:

```javascript
const headers = new Headers([["x-test", "a"]]);
headers.append("x-test", "b");
headers.set("content-type", "application/json");
```

Supported methods:

- `append(name, value)`
- `set(name, value)`
- `get(name)`
- `has(name)`
- `delete(name)`
- `forEach(callback)`
- iteration via `entries()`, `keys()`, `values()`, and `for...of`

### `Request`

`Request` accepts a URL-like input or another `Request` plus an optional init
object:

```javascript
const req = new Request("https://example.test/data", {
  method: "POST",
  body: { hello: "world" },
});
```

Supported members:

- `method`
- `url`
- `headers`
- `signal`
- `bodyUsed`
- `clone()`
- `text()`
- `json()`
- `arrayBuffer()`

### `Response`

`fetch(...)` resolves to a `Response`.

Supported members:

- `status`
- `statusText`
- `ok`
- `headers`
- `url`
- `bodyUsed`
- `clone()`
- `text()`
- `json()`
- `arrayBuffer()`

Response bodies are buffered. Once you consume a body with `text()`, `json()`,
or `arrayBuffer()`, `bodyUsed` becomes `true` and the body cannot be read again.

### Abort Support

Use `AbortController` and `AbortSignal` to cancel in-flight `fetch(...)`
operations:

```javascript
const controller = new AbortController();
controller.abort("stop");

try {
  await fetch("https://example.test/never", { signal: controller.signal });
} catch (err) {
  console.log(err.name); // AbortError
}
```

### URL Helpers

`URLSearchParams` is available for form-encoded bodies and query construction:

```javascript
const params = new URLSearchParams({ a: "1", b: "two" });
await fetch("https://example.test/form", {
  method: "POST",
  body: params,
});
```

`URL` objects are also accepted by `Request` and `fetch(...)`.

## Timers

The runtime provides timer globals backed by the sandbox clock:

- `setTimeout(callback, delayMs, ...args)`
- `clearTimeout(timerId)`
- `setInterval(callback, delayMs, ...args)`
- `clearInterval(timerId)`

These integrate with promises and `async` guest code in the same event loop used
by `hostcall(...)` and `fetch(...)`.

## Logging

The runtime injects a `console` object with:

- `console.log(...)`
- `console.debug(...)`
- `console.warn(...)`
- `console.error(...)`

These forward guest log messages to the host output sink with the matching log
level.
