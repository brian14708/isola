default: check

run: build
    cargo run --release -p promptkit_server

check: lint test

integration:
    uv run pytest

generate:
    cd ui && pnpm install && pnpm run generate
    cd tests/rpc && buf generate

build: build-wasm build-ui
    cargo build --release -p promptkit_server
    cargo run --release -p promptkit_server build

[working-directory('wasm')]
build-wasm:
    cargo xtask build-all

[working-directory('ui')]
build-ui:
    pnpm install
    pnpm run build

test: test-wasm
    cargo test

[working-directory('wasm')]
test-wasm:
    cargo test

lint: lint-wasm lint-ui lint-proto
    cargo clippy -- --deny warnings
    uv run ruff check
    uv run mypy tests/rpc

[working-directory('wasm')]
lint-wasm:
    cargo clippy -- --deny warnings
    cd crates/python/bundled && uv run mypy .

[working-directory('ui')]
lint-ui:
    pnpm install
    pnpm run lint

[working-directory('apis/proto')]
lint-proto:
    buf lint

integration-c:
    cmake -B target/c -G Ninja tests/c
    cmake --build target/c
    cmake --build target/c --target test
