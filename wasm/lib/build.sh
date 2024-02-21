#!/bin/bash -e
cd $(dirname "$0")
docker build -t wasm-libpython .
id=$(docker create wasm-libpython)
docker cp $id:/out/libpython.tar.gz .
docker rm -v $id
