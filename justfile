default: lint test

run: build
    cargo run --release -p isola-server

e2e: init-py build-wasm
    mkdir -p target
    sh -c 'PORT=3001 cargo run --release -p isola-server > target/e2e-server.log 2>&1 & pid=$!; trap "kill $pid >/dev/null 2>&1 || true" EXIT INT TERM; for _ in $(seq 1 60); do if curl -fsS http://127.0.0.1:3001/debug/healthz >/dev/null 2>&1; then break; fi; sleep 1; done; ISOLA_BASE_URL=http://127.0.0.1:3001 uv run --directory tests/rpc pytest'

integration:
    cargo test --release -p isola --test integration_python -- --ignored --test-threads=1

build: build-wasm
    cargo build --release -p isola-server
    cargo run --release -p isola-server build

[private]
build-wasm:
    cargo xtask build-all

test:
    cargo test --all-features

lint: init-py
    cargo clippy --all-features -- --deny warnings
    uv run ruff check
    uv run mypy
    uv run basedpyright

integration-c:
    cmake -B target/c -G Ninja tests/c
    cmake --build target/c
    cmake --build target/c --target test

[private]
init-py:
    uv sync --all-packages
