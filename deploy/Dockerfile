# syntax=docker/dockerfile:1

############################
# Stage 1: Rust server build
############################
FROM docker.io/chainguard/rust:latest-dev AS server-build
USER root
RUN apk add --no-cache protobuf-dev cmake glibc-dev
COPY crates crates
COPY specs specs
COPY Cargo.toml .
COPY Cargo.lock .
RUN --mount=type=cache,target=/work/target/release-lto \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    cargo b --profile release-lto -p promptkit-server && cp target/release-lto/promptkit-server target/promptkit-server

############################
# Stage 2: UI build
############################
FROM --platform=$BUILDPLATFORM docker.io/oven/bun:latest AS ui-build
WORKDIR /src
COPY ui/package.json ui/bun.lock .
RUN --mount=type=cache,target=/root/.bun/install/cache \
    bun install --frozen-lockfile
COPY ui .
COPY specs ../specs
RUN bun run build

############################
# Stage 3: Wasm build
############################
FROM --platform=$BUILDPLATFORM docker.io/library/rust:latest AS wasm-build
COPY wasm/rust-toolchain.toml .
RUN rustup show

RUN apt-get update && \
    apt-get install -y cmake ninja-build && \
    rm -rf /var/lib/apt/lists/*
RUN curl -sSL \
      https://github.com/WebAssembly/binaryen/releases/download/version_123/binaryen-version_123-$(uname -m)-linux.tar.gz \
      | tar -xz --strip-components=1 -C /usr/local/
ENV PYENV_ROOT=/root/.pyenv \
    PATH=/root/.pyenv/bin:/root/.pyenv/shims:$PATH \
    PYENV_GIT_TAG=v2.6.11
RUN curl -sSL https://pyenv.run | bash \
    && pyenv install 3.14.0 \
    && pyenv global 3.14.0
RUN pip install --root-user-action=ignore \
    cython wheel typing-extensions maturin setuptools

WORKDIR /src/wasm
COPY specs ../specs
COPY wasm .
RUN --mount=type=cache,target=/src/wasm/target/wasm32-wasip1/release \
    --mount=type=cache,target=/src/wasm/target/release \
    --mount=type=cache,target=/src/wasm/target/xtask \
    --mount=type=cache,target=/src/wasm/downloads \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo xtask build-all

RUN find /src/wasm/target/wasm32-wasip1/wasi-deps/usr/ -type f -name "*.so" -exec truncate -s 0 {} \;
RUN python -m compileall /src/wasm/target/wasm32-wasip1/wasi-deps/usr/

############################
# Stage 4: Staging (assemble)
############################
FROM docker.io/chainguard/glibc-dynamic:latest AS staging
WORKDIR /app
COPY --from=server-build --chown=nonroot:nonroot /work/target/promptkit-server .
COPY --from=ui-build --chown=nonroot:nonroot /src/dist ui/dist
COPY --from=wasm-build --chown=nonroot:nonroot /src/wasm/target/promptkit_python.wasm ./wasm/target/promptkit_python.wasm
COPY --from=wasm-build --chown=nonroot:nonroot /src/wasm/target/wasm32-wasip1/wasi-deps/usr ./wasm/target/wasm32-wasip1/wasi-deps/usr
RUN ["./promptkit-server", "build"]

############################
# Stage 5: Final runtime
############################
FROM docker.io/chainguard/glibc-dynamic:latest
WORKDIR /app
COPY --from=staging /app .
EXPOSE 3000/tcp
CMD ["./promptkit-server"]
