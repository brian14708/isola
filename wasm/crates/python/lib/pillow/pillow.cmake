include(FetchContent)
include(ExternalProject)

find_package(Python3 ${PYTHON_VERSION})

FetchContent_Declare(
  pillow-src
  URL "https://github.com/python-pillow/Pillow/archive/refs/tags/12.0.0.tar.gz"
  URL_HASH
    SHA256=9d24d8c1197610e63eee5e5cfa60025e4aaaba3f5c3c592e9a62cdf734f0bb13
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/pillow.patch)
FetchContent_MakeAvailable(pillow-src)

ExternalProject_Add(
  pillow-build
  PREFIX _deps/pillow
  SOURCE_DIR ${pillow-src_SOURCE_DIR}
  BUILD_IN_SOURCE TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND ""
  INSTALL_COMMAND
    cmake -E env PYTHONPATH=${WASMLIB_SYSROOT}/usr/local/lib/python3.14
    _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
    PKG_CONFIG_LIBDIR=${WASMLIB_SYSROOT}/lib/pkgconfig CC=${CMAKE_C_COMPILER}
    CXX=${CMAKE_CXX_COMPILER} AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
    CFLAGS=-fPIC\ -I${WASMLIB_SYSROOT}/usr/local/include/python3.14\ -I${WASMLIB_SYSROOT}/include/wasm32-wasip1
    LDFLAGS=-L${WASMLIB_SYSROOT}/lib/wasm32-wasip1\ ${CMAKE_BINARY_DIR}/libpython3.14.so\ -ldl\ ${WASMLIB_SYSROOT}/lib/libz.a
    -- ${Python3_EXECUTABLE} <SOURCE_DIR>/setup.py install
    --prefix=${CMAKE_BINARY_DIR}/pythonpkgs --single-version-externally-managed
    --root=/
  DEPENDS python zlib jpeg freetype)

install(DIRECTORY ${CMAKE_BINARY_DIR}/pythonpkgs/ DESTINATION usr/local)
