# syntax=docker/dockerfile:1

FROM cgr.dev/chainguard/rust:latest-dev AS server-build
USER root
RUN apk add --no-cache protobuf-dev cmake glibc-dev
COPY crates crates
COPY apis apis
COPY Cargo.toml .
COPY Cargo.lock .
RUN --mount=type=cache,target=/work/target/release \
    --mount=type=cache,target=/root/.cargo/registry \
    cargo b --release -p promptkit_server && cp target/release/promptkit_server target/promptkit_server

FROM --platform=$BUILDPLATFORM docker.io/library/node:latest AS ui-build
WORKDIR /src
COPY ui/package.json .
RUN npm install -g $(cat package.json | grep -o 'pnpm@[0-9.]\+')
COPY ui/pnpm-lock.yaml .
RUN --mount=type=cache,target=/root/.cache/pnpm/store \
    pnpm install --frozen-lockfile
COPY ui .
COPY apis ../apis
RUN pnpm run build

FROM --platform=$BUILDPLATFORM docker.io/library/rust:latest AS wasm-build
RUN rustup target add wasm32-wasip1

RUN apt-get update && \
    apt-get install -y cmake && \
    rm -rf /var/lib/apt/lists/*
ENV PYENV_ROOT=/root/.pyenv \
    PATH=/root/.pyenv/bin:/root/.pyenv/shims:$PATH
RUN curl https://pyenv.run | bash
RUN eval "$(pyenv init -)"
RUN pyenv install 3.13.0 && pyenv global 3.13.0

WORKDIR /src/wasm
COPY apis ../apis
COPY wasm .
RUN --mount=type=cache,target=/src/wasm/target/wasm32-wasip1/release \
    --mount=type=cache,target=/src/wasm/target/release \
    --mount=type=cache,target=/src/wasm/target/xtask \
    --mount=type=cache,target=/src/wasm/downloads \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo xtask build-all

FROM cgr.dev/chainguard/glibc-dynamic:latest AS staging
WORKDIR /app
COPY --from=server-build --chown=nonroot:nonroot /work/target/promptkit_server .
COPY --from=ui-build --chown=nonroot:nonroot /src/build ui/build
COPY --from=wasm-build --chown=nonroot:nonroot /src/wasm/target/promptkit_python.wasm ./wasm/target/promptkit_python.wasm
COPY --from=wasm-build --chown=nonroot:nonroot /src/wasm/target/wasm32-wasip1/wasi-deps/usr ./wasm/target/wasm32-wasip1/wasi-deps/usr
RUN ["./promptkit_server", "build"]

FROM cgr.dev/chainguard/glibc-dynamic:latest
WORKDIR /app
COPY --from=staging /app .
EXPOSE 3000/tcp
CMD ["./promptkit_server"]
