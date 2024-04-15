# syntax=docker/dockerfile:1

# server
FROM docker.io/library/rust:alpine AS server-build
RUN apk add --no-cache protobuf-dev musl-dev
WORKDIR /src
COPY crates crates
COPY apis apis
COPY Cargo.toml .
COPY Cargo.lock .
RUN --mount=type=cache,target=/src/target/release \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo b --release -p promptkit_server && cp target/release/promptkit_server target/promptkit_server

# ui
FROM docker.io/library/node:21-bookworm AS ui-build
WORKDIR /src
COPY ui/pnpm-workspace.yaml .
COPY ui/package.json .
COPY ui/packages/api/package.json packages/api/package.json
RUN npm install -g $(cat package.json | grep -o 'pnpm@[0-9.]\+')
COPY ui/pnpm-lock.yaml .
RUN --mount=type=cache,target=/root/.cache/pnpm/store \
    pnpm install --frozen-lockfile
COPY ui .
COPY apis ../apis
RUN pnpm run build

# wasm
FROM docker.io/library/rust:bookworm AS wasm-build
RUN rustup target add wasm32-wasi
RUN curl --location https://github.com/bytecodealliance/wizer/releases/download/v5.0.0/wizer-v5.0.0-x86_64-linux.tar.xz | \
    tar --strip-components 1 --directory /usr/local/bin --extract --xz
RUN curl --location https://github.com/WebAssembly/binaryen/releases/download/version_117/binaryen-version_117-x86_64-linux.tar.gz | \
    tar --strip-components 1 --directory /usr/local --extract --gunzip
RUN curl --location https://github.com/bytecodealliance/wasm-tools/releases/download/v1.203.0/wasm-tools-1.203.0-x86_64-linux.tar.gz | \
    tar --strip-components 1 --directory /usr/local/bin --extract --gunzip
RUN curl --location https://github.com/sagiegurari/cargo-make/releases/download/0.37.10/cargo-make-v0.37.10-x86_64-unknown-linux-gnu.zip -o cargo-make.zip && \
    unzip -o -X -j cargo-make.zip -d /usr/local/bin && \
    rm cargo-make.zip
WORKDIR /src/wasm
COPY apis ../apis
COPY wasm .
RUN --mount=type=cache,target=/src/wasm/target/wasm32-wasi/wasi-deps/downloads \
    --mount=type=cache,target=/src/wasm/target/release \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo fetch && cargo make wasm

# final
FROM cgr.dev/chainguard/static:latest
WORKDIR /app
USER nonroot
COPY --from=server-build /src/target/promptkit_server .
COPY --from=ui-build /src/dist ui/dist
COPY --from=wasm-build --chown=nonroot:nonroot /src/wasm/target/promptkit_python.wasm ./wasm/target/promptkit_python.wasm
COPY --from=wasm-build /src/wasm/target/wasm32-wasi/wasi-deps/usr ./wasm/target/wasm32-wasi/wasi-deps/usr
RUN ["./promptkit_server", "build"]

EXPOSE 3000
CMD ["./promptkit_server"]
