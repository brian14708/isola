include(ExternalProject)

ExternalProject_Add(
  zlib-build
  URL "https://www.zlib.net/zlib-1.3.1.tar.gz"
  URL_HASH
    SHA256=9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23
  PREFIX _deps/zlib
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  CONFIGURE_COMMAND
    CC=${CMAKE_C_COMPILER} prefix=/ AR=${CMAKE_AR} RANLIB=${CMAKE_RANLIB}
    CHOST=wasm32-wasi <SOURCE_DIR>/configure --static
  INSTALL_COMMAND DESTDIR=<INSTALL_DIR> make install)

add_library(zlib STATIC IMPORTED GLOBAL)
add_dependencies(zlib zlib-build)
set_target_properties(zlib PROPERTIES IMPORTED_LOCATION
                                      ${WASMLIB_SYSROOT}/lib/libz.a)
