default: lint test

run: build
    cargo run --release -p promptkit-server

integration: init-py
    uv run --directory tests/rpc pytest

generate: init-ui
    cd ui && bun run generate
    cd tests/rpc && buf generate

build: build-wasm build-ui
    cargo build --release -p promptkit-server
    cargo run --release -p promptkit-server build

[private]
[working-directory('wasm')]
build-wasm:
    cargo xtask build-all

[private]
[working-directory('ui')]
build-ui: init-ui
    bun install
    bun run build

test: test-wasm
    cargo test --all-features

[private]
[working-directory('wasm')]
test-wasm:
    cargo test --all-features

lint: lint-wasm lint-ui lint-proto init-py
    cargo clippy --all-features -- --deny warnings
    uv run ruff check

[private]
lint-wasm: init-py
    cd wasm && cargo clippy --all-features -- --deny warnings
    uv run mypy wasm/crates/python/bundled
    uv run basedpyright -p wasm/crates/python/bundled

[private]
[working-directory('ui')]
lint-ui: init-ui
    bun run lint

[private]
[working-directory('specs/proto')]
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
    bun install --frozen
