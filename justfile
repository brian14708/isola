default: lint test

integration: build-wasm
    cargo test --release -p isola --test integration

build: build-wasm

docs:
    uv run --group docs zensical build

docs-serve:
    uv run --group docs zensical serve

[private]
build-wasm:
    cargo xtask build-all

test:
    cargo test --all-features

lint: init-py lint-rust lint-python lint-js

lint-rust:
    cargo clippy --all-features -- --deny warnings

lint-python: init-py
    uv run ruff check --config crates/python-runtime/pyproject.toml crates/python-runtime/python
    uv run ruff check --config crates/python-sdk/pyproject.toml crates/python-sdk
    uv run mypy --config-file crates/python-runtime/pyproject.toml
    uv run mypy --config-file crates/python-sdk/pyproject.toml
    uv run basedpyright --project crates/python-runtime/pyproject.toml
    uv run basedpyright --project crates/python-sdk/pyproject.toml

lint-js: init-js
    pnpm --filter isola-sdk tsc --noEmit

build-js: init-js
    pnpm --filter isola-sdk run build

vitest: build-js
    pnpm --filter isola-sdk exec env ISOLA_RUNTIME_PATH=../../target pnpm vitest run

[private]
init-js:
    pnpm install

pytest: init-py
    cd ./crates/python-sdk/ && maturin develop --release
    uv run pytest ./crates/python-sdk/tests/

integration-c:
    cmake -B target/c -G Ninja crates/c-api/tests
    cmake --build target/c
    cmake --build target/c --target test

[private]
init-py:
    uv sync --all-packages --no-install-project
