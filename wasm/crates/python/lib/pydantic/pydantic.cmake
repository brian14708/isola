include(FetchContent)
include(ExternalProject)

find_package(Python3 ${PYTHON_VERSION})

FetchContent_Declare(
  pydantic-src
  URL "https://github.com/pydantic/pydantic-core/archive/refs/tags/v2.41.1.tar.gz"
  URL_HASH
    SHA256=7ea0323d518f49dfe5619b90547529c0e18791e77a114040cac9d08fa373339a
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/pydantic.patch)
FetchContent_MakeAvailable(pydantic-src)

ExternalProject_Add(
  pydantic-build
  PREFIX _deps/pydantic
  SOURCE_DIR ${pydantic-src_SOURCE_DIR}
  BUILD_IN_SOURCE TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND
    cmake -E env --unset=CARGO_ENCODED_RUSTFLAGS
    CARGO_TARGET_WASM32_WASIP1_LINKER=${WASI_SDK_PATH}/bin/wasm-ld
    PYTHONPATH=${WASMLIB_SYSROOT}/usr/local/lib/python3.14
    _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
    PYO3_CROSS_LIB_DIR=${WASMLIB_SYSROOT}/usr/local/lib/python3.14
    CC=${CMAKE_C_COMPILER} AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
    LDSHARED=${CMAKE_C_COMPILER}
    "RUSTFLAGS=-Clink-args=-L${CMAKE_BINARY_DIR} -Clink-args=-L${WASMLIB_SYSROOT}/lib/wasm32-wasip1 -Clink-self-contained=no -Crelocation-model=pic"
    -- maturin build --release --target wasm32-wasip1 -i python3.14 --out dist
  INSTALL_COMMAND
    mkdir -p ${CMAKE_BINARY_DIR}/pythonpkgs/lib/python3.14/site-packages && cd
    ${CMAKE_BINARY_DIR}/pythonpkgs/lib/python3.14/site-packages && cmake -E tar
    xvf <SOURCE_DIR>/dist/pydantic_core-2.41.1-cp314-cp314-any.whl
  DEPENDS python)
