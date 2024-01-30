# server
FROM docker.io/library/rust:bookworm AS server-build
WORKDIR /src
COPY crates crates
COPY Cargo.toml .
COPY Cargo.lock .
RUN --mount=type=cache,target=/src/target/release \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo b --release -p promptkit_server && cp target/release/promptkit_server target/promptkit_server

# ui
FROM docker.io/library/node:20-bookworm AS ui-build
WORKDIR /src
COPY ui/package.json .
RUN npm install -g $(cat package.json | grep -o 'pnpm@[0-9.]\+')
COPY ui/pnpm-lock.yaml .
RUN --mount=type=cache,target=/root/.cache/pnpm/store \
    pnpm install --frozen-lockfile
COPY ui .
RUN pnpm run build

# wasm
FROM docker.io/library/rust:bookworm AS wasm-build
RUN rustup target add wasm32-wasi
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install cargo-make --locked
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install wizer --all-features --locked
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install wasm-opt --locked
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install wasm-tools --locked
WORKDIR /src
COPY wasm .
RUN --mount=type=cache,target=/src/target/wasm32-wasi \
    --mount=type=cache,target=/src/target/release-wasm \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo make wasm

# final
FROM gcr.io/distroless/cc-debian12
WORKDIR /app
USER nonroot
COPY --from=server-build --chown=nonroot:nonroot /src/target/promptkit_server .
COPY --from=ui-build --chown=nonroot:nonroot /src/dist ui/dist
COPY --from=wasm-build --chown=nonroot:nonroot /src/target/promptkit_python.wasm ./wasm/target/promptkit_python.wasm

EXPOSE 3000
CMD ["./promptkit_server"]
