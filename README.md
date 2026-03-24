# 🏝️ Isola

[![crates.io](https://img.shields.io/crates/v/isola?logo=rust)](https://crates.io/crates/isola)
[![PyPI](https://img.shields.io/pypi/v/isola?logo=python&logoColor=white)](https://pypi.org/project/isola/)
[![npm](https://img.shields.io/npm/v/isola-core?logo=npm)](https://www.npmjs.com/package/isola-core)
[![License](https://img.shields.io/badge/license-Apache%202.0-2563eb)](LICENSE)

Isola is a Rust runtime, with Python and Node.js SDKs, for running untrusted
Python and JavaScript inside reusable WebAssembly sandboxes.

It aims to sit between two common approaches:

- embedding an interpreter directly into your process, which is flexible but
  gives you a weaker sandbox boundary
- starting a container or microVM for every execution, which is isolated but
  heavier and more operationally expensive

In practice, that means compiling a reusable sandbox template once, then
instantiating isolated sandboxes with explicit policy around memory,
filesystem mounts, environment variables, outbound HTTP, and host callbacks.

## ⚡ Quick Start

If you just want to see Isola run, install the Python SDK:

```bash
pip install isola
```

```python
import asyncio

from isola import build_template


async def main() -> None:
    # First run may take a few seconds: build_template(...) downloads the
    # runtime from GitHub Releases, verifies it, caches it, and precompiles
    # a reusable template.
    template = await build_template("python")

    async with template.create(http_handler=True) as sandbox:
        await sandbox.load_script(
            "from sandbox.http import fetch\n"
            "\n"
            "async def main(url):\n"
            "    async with fetch('GET', url) as resp:\n"
            "        return await resp.ajson()\n"
        )
        print(await sandbox.run("main", "https://httpbin.org/get"))


asyncio.run(main())
```

## 🎯 Potential Good Fits

- AI code execution, where an agent writes short Python or JavaScript helpers
  and the host decides what external effects are allowed
- Multi-tenant plugin systems that want per-request sandboxes without starting a
  container for every call
- User-authored automation or ETL steps that need streaming outputs and
  policy-wrapped access to internal services
- Internal extension points where teams want Python or JavaScript ergonomics
  without giving scripts full host access

## 🔍 Compared With Other Approaches

- Versus embedded interpreters: Isola tries to offer a more explicit Wasm
  sandbox boundary and deny-by-default host access instead of ambient
  in-process access.
- Versus componentization workflows such as `componentize-py` or ComponentizeJS:
  Isola accepts raw source at runtime rather than requiring a per-script build
  step and a fixed interface.
- Versus containers, microVMs, or managed subhosting: Isola can be lighter to
  embed in your own service, but it is not a full Linux environment and does
  not try to replace stronger infrastructure isolation.

## 🚫 When Isola May Not Be The Right Tool

- You need arbitrary native extensions, subprocesses, or a full Linux userspace
- You want a fixed guest contract compiled ahead of time as a Wasm component
- Your code is trusted and you mainly want the lowest-overhead way to embed a
  language runtime into your process
- You need infrastructure-level isolation or managed hosting rather than an
  embeddable runtime library

## 🔐 Security Model

Isola is intended for untrusted guest code, but it is still a library you embed
inside your own process. The host surface is explicit and should stay
small: mounts, environment variables, HTTP handlers, and hostcalls are the main
places where policy mistakes can expose capabilities. Treat those boundaries as
part of your application security model.
