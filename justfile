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

bundle-wasm: build-wasm
    mkdir -p dist/wasm/
    cp wasm/target/promptkit_python.wasm dist/wasm/
    cp -r wasm/target/wasm32-wasip1/wasi-deps/usr/local/lib dist/wasm/lib
    find dist/wasm/ -type f -name "*.so" -exec truncate -s 0 {} \;
    python -m compileall dist/wasm/ -q
    tar -czf dist/promptkit_wasm_bundle.tar.gz -C dist/ wasm

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
    uv run --directory wasm/crates/python/bundled mypy .
    uv run --directory wasm/crates/python/bundled basedpyright .

[private]
[working-directory('ui')]
lint-ui: init-ui
    bun run lint

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
    bun install --frozen
