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

lint: init-py lint-rust lint-python

lint-rust:
    cargo clippy --all-features -- --deny warnings

lint-python: init-py
    uv run ruff check --config crates/python/pyproject.toml crates/python/bundled
    uv run ruff check --config crates/py-binding/pyproject.toml crates/py-binding/python crates/py-binding/tests
    uv run mypy --config-file crates/python/pyproject.toml
    uv run mypy --config-file crates/py-binding/pyproject.toml
    uv run basedpyright --project crates/python/pyproject.toml
    uv run basedpyright --project crates/py-binding/pyproject.toml

pytest: init-py
    uv run pytest ./crates/py-binding/tests/

[private]
init-py:
    uv sync --all-packages
