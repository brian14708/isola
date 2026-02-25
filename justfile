default: lint test

run: build
    cargo run --release -p isola-server

integration: build-wasm
    cargo test --release -p isola --test integration

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

lint: init-py lint-rust lint-python

lint-rust:
    cargo clippy --all-features -- --deny warnings

lint-python: init-py
    uv run ruff check --config crates/python-runtime/pyproject.toml crates/python-runtime/python
    uv run ruff check --config crates/python-sdk/pyproject.toml crates/python-sdk
    uv run mypy --config-file crates/python-runtime/pyproject.toml
    uv run mypy --config-file crates/python-sdk/pyproject.toml
    uv run basedpyright --project crates/python-runtime/pyproject.toml
    uv run basedpyright --project crates/python-sdk/pyproject.toml

pytest: init-py
    uv run pytest ./crates/python-sdk/tests/

integration-c:
    cmake -B target/c -G Ninja crates/c-api/tests
    cmake --build target/c
    cmake --build target/c --target test

[private]
init-py:
    uv sync --all-packages
