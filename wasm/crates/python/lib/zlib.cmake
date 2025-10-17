include(ExternalProject)

ExternalProject_Add(
  zlib-build
  URL "https://github.com/zlib-ng/zlib-ng/archive/refs/tags/2.2.5.tar.gz"
  URL_HASH
    SHA256=5b3b022489f3ced82384f06db1e13ba148cbce38c7941e424d6cb414416acd18
  PREFIX _deps/zlib
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  CMAKE_ARGS -DCMAKE_BUILD_TYPE=Release
             -DCMAKE_TOOLCHAIN_FILE=${CMAKE_TOOLCHAIN_FILE}
             -DCMAKE_INSTALL_PREFIX=<INSTALL_DIR>
             -DCMAKE_POSITION_INDEPENDENT_CODE=ON
             -DZLIB_COMPAT=ON
             -DZLIB_ENABLE_TESTS=OFF
             -DBUILD_SHARED_LIBS=OFF
             -DWITH_RUNTIME_CPU_DETECTION=OFF)

add_library(zlib STATIC IMPORTED GLOBAL)
add_dependencies(zlib zlib-build)
set_target_properties(zlib PROPERTIES IMPORTED_LOCATION
                                      ${WASMLIB_SYSROOT}/lib/libz.a)
