---
title: Home
hide:
  - toc
---

# Isola

Isola provides a sandboxed Rust runtime plus Python and Node.js SDKs for
running untrusted workloads with explicit resource and environment controls.

The documentation is split by where code runs:

- Host APIs cover the Python `isola` SDK and the Node.js `isola-core` SDK used
  to compile templates, create sandboxes, and configure hostcalls or HTTP
  policy.
- Guest APIs cover the Python `sandbox.*` modules and the JavaScript globals
  available inside sandboxed code.
