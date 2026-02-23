default: lint test

run: build
    cargo run --release -p isola-server

integration: build-wasm
    cargo test --release -p isola integration_python

build: build-wasm
    cargo build --release -p isola-server
    cargo run --release -p isola-server build

docs:
    mdbook build docs

[private]
build-wasm:
    cargo xtask build-all

test:
    cargo test --all-features

lint: init-py
    cargo clippy --all-features -- --deny warnings
    uv run ruff check
    uv run mypy
    uv run basedpyright

[private]
init-py:
    uv sync --all-packages
