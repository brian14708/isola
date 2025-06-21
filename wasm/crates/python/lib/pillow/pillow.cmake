include(FetchContent)
include(ExternalProject)

find_package(Python3 ${PYTHON_VERSION})

FetchContent_Declare(
  pillow-src
  URL "https://github.com/python-pillow/Pillow/archive/refs/tags/11.2.1.tar.gz"
  URL_HASH
    SHA256=67dd6d40e282fd3b50189bd7f68a6e9b8e59b297d3684cb35ab107e7aec8379e
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND
    patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/pillow.patch
)
FetchContent_MakeAvailable(pillow-src)

ExternalProject_Add(
  pillow-build
  PREFIX _deps/pillow
  SOURCE_DIR ${pillow-src_SOURCE_DIR}
  BUILD_IN_SOURCE TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND ""
  INSTALL_COMMAND
    cmake -E env
      PYTHONPATH=${WASMLIB_SYSROOT}/usr/local/lib/python3.13
      _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
      PKG_CONFIG_LIBDIR=${WASMLIB_SYSROOT}/lib/pkgconfig
      CC=${CMAKE_C_COMPILER} CXX=${CMAKE_CXX_COMPILER} AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
      CFLAGS=-fPIC\ -I${WASMLIB_SYSROOT}/usr/local/include/python3.13\ -I${WASMLIB_SYSROOT}/include/wasm32-wasip1
      LDFLAGS=-L${WASMLIB_SYSROOT}/lib/wasm32-wasip1\ ${CMAKE_BINARY_DIR}/libpython3.13.so\ -ldl
    --
    ${Python3_EXECUTABLE} <SOURCE_DIR>/setup.py
      install --prefix=${CMAKE_BINARY_DIR}/pythonpkgs
      --single-version-externally-managed --root=/
  DEPENDS python zlib jpeg freetype)

install(DIRECTORY ${CMAKE_BINARY_DIR}/pythonpkgs/ DESTINATION usr/local)
