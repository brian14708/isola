# Isola

Isola runs untrusted Python and JavaScript in WebAssembly sandboxes. Each sandbox gets explicit memory limits, scoped filesystem mounts, and a controlled HTTP layer — so guest code can only do what the host explicitly allows.

Common uses:

- **AI code execution** — let a model write a script and run it safely, with hostcall callbacks as the only way to reach your services
- **User-submitted code** — multi-tenant notebooks or automation builders with strong per-run isolation
- **Plugin systems** — compile a template once per plugin, instantiate cheaply per request, discard when done
- **Streaming pipelines** — guest `yield` streams values back over SSE or Python async iteration
