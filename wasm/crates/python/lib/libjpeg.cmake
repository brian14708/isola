include(ExternalProject)

ExternalProject_Add(
  jpeg-build
  URL "https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/3.1.0.tar.gz"
  URL_HASH
    SHA256=35fec2e1ddfb05ecf6d93e50bc57c1e54bc81c16d611ddf6eff73fff266d8285
  PREFIX _deps/jpeg
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  CONFIGURE_COMMAND
    ${CMAKE_COMMAND} <SOURCE_DIR> -DCMAKE_BUILD_TYPE=Release -DCMAKE_TOOLCHAIN_FILE=${CMAKE_TOOLCHAIN_FILE} -DWITH_TURBOJPEG=OFF -DCMAKE_INSTALL_PREFIX=<INSTALL_DIR> -DENABLE_SHARED=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DCMAKE_C_FLAGS=-mllvm\ -wasm-enable-sjlj
  PATCH_COMMAND
    patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/libjpeg.patch
)

add_library(jpeg STATIC IMPORTED GLOBAL)
add_dependencies(jpeg jpeg-build)
