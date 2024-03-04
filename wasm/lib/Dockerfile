FROM docker.io/library/fedora:41

ENV CC=clang
ENV WASI_SDK_PATH=/opt/wasi-sdk
ENV WASMTIME_HOME=/opt/wasmtime

RUN dnf -y --nodocs --setopt=install_weak_deps=False install /usr/bin/{blurb,clang,curl,git,ln,tar,xz} 'dnf-command(builddep)' && \
    dnf -y --nodocs --setopt=install_weak_deps=False builddep python3 && \
    dnf -y clean all

ARG WASI_SDK_VERSION=21
RUN mkdir ${WASI_SDK_PATH} && \
    curl --location https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/wasi-sdk-${WASI_SDK_VERSION}.0-linux.tar.gz | \
    tar --strip-components 1 --directory ${WASI_SDK_PATH} --extract --gunzip

ARG WASMTIME_VERSION=18.0.1
RUN mkdir --parents ${WASMTIME_HOME} && \
    curl --location "https://github.com/bytecodealliance/wasmtime/releases/download/v${WASMTIME_VERSION}/wasmtime-v${WASMTIME_VERSION}-$(uname -m)-linux.tar.xz" | \
    xz --decompress | \
    tar --strip-components 1 --directory ${WASMTIME_HOME} -x && \
    ln -s ${WASMTIME_HOME}/wasmtime /usr/local/bin

WORKDIR /src
ARG ZLIB_VERSION=1.3.1
RUN curl https://www.zlib.net/zlib-${ZLIB_VERSION}.tar.gz | tar --directory . --extract --gunzip
RUN cd zlib-${ZLIB_VERSION} && \
    prefix=${WASI_SDK_PATH}/share/wasi-sysroot libdir=${WASI_SDK_PATH}/share/wasi-sysroot/lib/wasm32-wasi pkgconfigdir= AR="${WASI_SDK_PATH}/bin/llvm-ar" RANLIB="${WASI_SDK_PATH}/bin/llvm-ranlib" CC="${WASI_SDK_PATH}/bin/clang --sysroot=${WASI_SDK_PATH}/share/wasi-sysroot" ./configure && \
    make && make install && \
    mkdir -p ${WASI_SDK_PATH}/share/wasi-sysroot/lib/pkgconfig && \
    mv ${WASI_SDK_PATH}/share/wasi-sysroot/lib/wasm32-wasi/pkgconfig/* ${WASI_SDK_PATH}/share/wasi-sysroot/lib/pkgconfig

ARG PYTHON_VERSION=3.12.2
RUN curl https://www.python.org/ftp/python/${PYTHON_VERSION}/Python-${PYTHON_VERSION}.tar.xz | tar -xJ
COPY libpython.patch .
RUN cd Python-${PYTHON_VERSION} && git apply --allow-empty ../libpython.patch
RUN cd Python-${PYTHON_VERSION} && python3 ./Tools/wasm/wasm_build.py wasi 
RUN cd Python-${PYTHON_VERSION}/builddir/wasi && _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi python3 ../../Tools/wasm/wasm_assets.py

WORKDIR /out
RUN mkdir -p lib/wasm32-wasi lib/wasi include/python$(echo ${PYTHON_VERSION} | awk -F. '{print $1 "." $2}') && \
    cp -r /src/Python-${PYTHON_VERSION}/Include/* include/python$(echo ${PYTHON_VERSION} | awk -F. '{print $1 "." $2}') && \
    cp -r /src/Python-${PYTHON_VERSION}/builddir/wasi/usr . && \
    echo "CREATE lib/wasm32-wasi/libpython$(echo ${PYTHON_VERSION} | awk -F. '{print $1 "." $2}').a" >> lib.def && \
    echo "ADDLIB ${WASI_SDK_PATH}/share/wasi-sysroot/lib/wasm32-wasi/libz.a" >> lib.def && \
    find /src/Python-${PYTHON_VERSION}/builddir/wasi/ -name '*.a' -exec echo ADDLIB {} >> lib.def \; && \
    echo "SAVE" >> lib.def && \
    ${WASI_SDK_PATH}/bin/llvm-ar -M < lib.def && \
    cp ${WASI_SDK_PATH}/lib/clang/*/lib/wasi/libclang_rt.builtins-wasm32.a lib/wasi && \
    cp -r ${WASI_SDK_PATH}/share/wasi-sysroot . && \
    tar --sort=name --mtime='1970-01-01 00:00Z' --owner=0 --group=0 --numeric-owner -c wasi-sysroot lib usr include | gzip -n --best > libpython.tar.gz && \
    rm -rf lib usr include lib.def
