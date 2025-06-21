include(ExternalProject)

ExternalProject_Add(
  jpeg-build
  URL "https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/3.1.1.tar.gz"
  URL_HASH
    SHA256=304165ae11e64ab752e9cfc07c37bfdc87abd0bfe4bc699e59f34036d9c84f72
  PREFIX _deps/jpeg
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  CMAKE_ARGS
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_TOOLCHAIN_FILE=${CMAKE_TOOLCHAIN_FILE} -DWITH_TURBOJPEG=OFF -DCMAKE_INSTALL_PREFIX=<INSTALL_DIR> -DENABLE_SHARED=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DCMAKE_C_FLAGS=-mllvm\ -wasm-enable-sjlj
  PATCH_COMMAND
    patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/libjpeg.patch
)

add_library(jpeg STATIC IMPORTED GLOBAL)
add_dependencies(jpeg jpeg-build)
