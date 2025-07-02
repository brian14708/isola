include(ExternalProject)

ExternalProject_Add(
  freetype-build
  URL "https://download.savannah.gnu.org/releases/freetype/freetype-2.13.3.tar.xz"
  URL_HASH
    SHA256=0550350666d427c74daeb85d5ac7bb353acba5f76956395995311a9c6f063289
  PREFIX _deps/freetype
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  PATCH_COMMAND patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/freetype.patch
  CONFIGURE_COMMAND
    cmake -E env CC=${CMAKE_C_COMPILER} CXX=${CMAKE_CXX_COMPILER}
    AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
    CFLAGS=-fPIC\ -I${WASMLIB_SYSROOT}/include/wasm32-wasip1
    <SOURCE_DIR>/configure --prefix=<INSTALL_DIR> --host=wasm32-wasi
    --with-brotli=no --with-bzip2=no --with-zlib=no --with-png=no
    --with-harfbuzz=no)

add_library(freetype STATIC IMPORTED GLOBAL)
add_dependencies(freetype freetype-build)
