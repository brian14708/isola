default: lint test

run: build
    cargo run --release -p promptkit-server

integration: init-py
    uv run --directory tests/rpc pytest

generate: init-ui
    cd ui && pnpm run generate
    cd tests/rpc && buf generate

build: build-wasm build-ui
    cargo build --release -p promptkit-server
    cargo run --release -p promptkit-server build

[private]
build-wasm:
    cargo xtask build-all

[private]
[working-directory('ui')]
build-ui: init-ui
    pnpm install
    pnpm run build

test:
    cargo test --all-features

lint: lint-ui lint-proto init-py
    cargo clippy --all-features -- --deny warnings
    uv run ruff check
    uv run mypy
    uv run basedpyright

[private]
[working-directory('ui')]
lint-ui: init-ui
    pnpm run lint

[private]
[working-directory('specs/grpc')]
lint-proto:
    buf lint

integration-c:
    cmake -B target/c -G Ninja tests/c
    cmake --build target/c
    cmake --build target/c --target test

[private]
init-py:
    uv sync --all-packages

[private]
[working-directory('ui')]
init-ui:
    pnpm install --frozen
