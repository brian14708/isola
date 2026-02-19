default: lint test

run: build
    cargo run --release -p isola-server

e2e: init-py build-wasm
    uv run --directory tests/rpc pytest

integration:
    cargo test --release -p isola --test integration_python -- --ignored --test-threads=1

build: build-wasm
    cargo build --release -p isola-server
    cargo run --release -p isola-server build

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
    cmake -B target/c -G Ninja tests/c
    cmake --build target/c
    cmake --build target/c --target test

[private]
init-py:
    uv sync --all-packages
