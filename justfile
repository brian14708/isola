default:
    @just --list

run: build
    cargo run --release

check: lint test

build: build-wasm build-ui
    cargo build --release
    cargo run --release build

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
    cargo clippy
    uv run ruff check
    uv run ruff format --check

[working-directory('wasm')]
lint-wasm:
    cargo clippy

[working-directory('ui')]
lint-ui:
    pnpm install
    pnpm run lint

[working-directory('apis/proto')]
lint-proto:
    buf lint
    buf format --exit-code --diff
