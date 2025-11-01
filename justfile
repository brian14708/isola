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
    rm -rf dist/python/
    mkdir -p dist/python/
    cp wasm/target/promptkit_python.wasm dist/python/
    cp -r wasm/target/wasm32-wasip1/wasi-deps/usr/local/lib dist/python/lib
    find dist/python/ -type f -name "*.so" -exec truncate -s 0 {} \;
    rm -f dist/python/lib/bundle-src.zip
    python -m compileall dist/python/ -q
    tar -czf dist/promptkit-python.tar.gz -C dist/ python

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
    uv run mypy
    uv run basedpyright

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
