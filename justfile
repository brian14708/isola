default: lint test

run: build
    cargo run --release -p isola-server

integration: build-wasm
    cargo test --release -p isola

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

integration-c:
    cmake -B target/c -G Ninja crates/c-api/tests
    cmake --build target/c
    cmake --build target/c --target test

[private]
init-py:
    uv sync --all-packages
