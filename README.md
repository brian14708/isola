# Isola

## Documentation

Documentation is authored with mdBook in `docs/`.

- Build locally: `just docs`
- Serve locally: `just docs-serve`
- Deploy: automatically published to GitHub Pages from the `Docs` workflow on
  `main`

## Build from source

Execute the following commands to build and launch Isola:

```
nix develop
cargo xtask build-python
cargo run -p isola-server
```
