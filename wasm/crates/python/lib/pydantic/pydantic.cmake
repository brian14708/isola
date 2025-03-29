include(FetchContent)
include(ExternalProject)

find_package(Python3 ${PYTHON_VERSION})

FetchContent_Declare(
  pydantic-src
  URL "https://github.com/pydantic/pydantic-core/archive/refs/tags/v2.33.0.tar.gz"
  URL_HASH
    SHA256=8ddef852cf968f3b5a857db1749f898ac5209deea78681dc1b8ae018341dbf27
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND
    patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/pydantic.patch
)
FetchContent_MakeAvailable(pydantic-src)

ExternalProject_Add(
  pydantic-build
  PREFIX _deps/pydantic
  SOURCE_DIR ${pydantic-src_SOURCE_DIR}
  BUILD_IN_SOURCE TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND
    cmake -E env
      --unset=CARGO_ENCODED_RUSTFLAGS
      CARGO_TARGET_WASM32_WASIP1_LINKER=${WASI_SDK_PATH}/bin/wasm-ld
      PYTHONPATH=${WASMLIB_SYSROOT}/usr/local/lib/python3.13
      _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
      PYO3_CROSS_LIB_DIR=${WASMLIB_SYSROOT}/usr/local/lib
      CC=${CMAKE_C_COMPILER} AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
      LDSHARED=${CMAKE_C_COMPILER}
      "RUSTFLAGS=-Clink-args=-L${CMAKE_BINARY_DIR} -Clink-args=-L${WASMLIB_SYSROOT}/lib/wasm32-wasip1 -Clink-self-contained=no -Crelocation-model=pic" --
    maturin build --release --target wasm32-wasip1 -i python3.13 --out dist
  INSTALL_COMMAND
    cd ${CMAKE_BINARY_DIR}/pythonpkgs/lib/python3.13/site-packages && cmake -E tar xvf <SOURCE_DIR>/dist/pydantic_core-2.33.0-cp313-cp313-any.whl
  DEPENDS python)
