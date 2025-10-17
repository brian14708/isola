include(ExternalProject)

ExternalProject_Add(
  freetype-build
  URL "https://download.savannah.gnu.org/releases/freetype/freetype-2.14.1.tar.xz"
  URL_HASH
    SHA256=32427e8c471ac095853212a37aef816c60b42052d4d9e48230bab3bdf2936ccc
  PREFIX _deps/freetype
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  PATCH_COMMAND patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/freetype.patch
  CONFIGURE_COMMAND
    cmake -E env CC=${CMAKE_C_COMPILER} CC_BUILD=$ENV{CC}
    CXX=${CMAKE_CXX_COMPILER} AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
    LD=${CMAKE_LINKER} CFLAGS=-fPIC ZLIB_CFLAGS=-I${WASMLIB_SYSROOT}/include
    <SOURCE_DIR>/configure --prefix=<INSTALL_DIR> --host=wasm32-wasi
    --with-brotli=no --with-bzip2=no --with-png=no
    --with-sysroot=${WASMLIB_SYSROOT} --with-harfbuzz=no
  DEPENDS zlib)

add_library(freetype STATIC IMPORTED GLOBAL)
add_dependencies(freetype freetype-build)
