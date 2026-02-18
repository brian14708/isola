default: lint test

run: build
    cargo run --release -p isola-server

e2e: init-py build-wasm
    mkdir -p target
    sh -c 'PORT=3001 cargo run --release -p isola-server > target/e2e-server.log 2>&1 & pid=$!; trap "kill $pid >/dev/null 2>&1 || true" EXIT INT TERM; for _ in $(seq 1 60); do if curl -fsS http://127.0.0.1:3001/debug/healthz >/dev/null 2>&1; then break; fi; sleep 1; done; PROMPTKIT_BASE_URL=http://127.0.0.1:3001 uv run --directory tests/rpc pytest'

integration:
    cargo test --release -p isola --test integration_python -- --ignored --test-threads=1

generate: init-ui
    cd ui && pnpm run generate
    cd tests/rpc && buf generate

build: build-wasm build-ui
    cargo build --release -p isola-server
    cargo run --release -p isola-server build

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
