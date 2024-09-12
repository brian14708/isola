include(FetchContent)
include(ExternalProject)

set(PYTHON_VERSION 3.12)
FetchContent_Declare(
  python-src
  URL "https://www.python.org/ftp/python/3.12.6/Python-3.12.6.tar.xz"
  URL_HASH
    SHA256=1999658298cf2fb837dffed8ff3c033ef0c98ef20cf73c5d5f66bed5ab89697c
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR})
FetchContent_MakeAvailable(python-src)

execute_process(
  COMMAND ${python-src_SOURCE_DIR}/config.guess
  OUTPUT_VARIABLE PYTHON_BUILD_ARCH
  OUTPUT_STRIP_TRAILING_WHITESPACE)

find_package(Python3 ${PYTHON_VERSION} EXACT)

ExternalProject_Add(
  python-build
  PREFIX _deps/python
  SOURCE_DIR ${python-src_SOURCE_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  CONFIGURE_COMMAND
    CONFIG_SITE=<SOURCE_DIR>/Tools/wasm/config.site-wasm32-wasi
    WASI_SDK_PATH=${WASI_SDK_PATH} <SOURCE_DIR>/Tools/wasm/wasi-env
    <SOURCE_DIR>/configure
      ac_cv_func_dlopen=no
      --prefix=/usr/local --host=wasm32-wasi
      --build=${PYTHON_BUILD_ARCH} --with-build-python=${Python3_EXECUTABLE}
      --disable-test-modules --with-pymalloc --with-computed-gotos --with-lto=thin
  INSTALL_COMMAND DESTDIR=<INSTALL_DIR> make install
  DEPENDS zlib)
ExternalProject_Add_Step(
  python-build assets
  DEPENDEES install
  COMMAND
    _PYTHON_HOST_PLATFORM=wasi-wasm32
    _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
    ${Python3_EXECUTABLE} <SOURCE_DIR>/Tools/wasm/wasm_assets.py
  WORKING_DIRECTORY <BINARY_DIR>)
ExternalProject_Get_Property(python-build BINARY_DIR)
install(DIRECTORY ${BINARY_DIR}/usr/ DESTINATION usr)
add_library(python STATIC IMPORTED GLOBAL)
add_dependencies(python python-build)
set_target_properties(
  python
  PROPERTIES IMPORTED_LOCATION ${WASMLIB_SYSROOT}/usr/local/lib/libpython${PYTHON_VERSION}.a
             INTERFACE_INCLUDE_DIRECTORIES
             ${WASMLIB_SYSROOT}/usr/local/include/python${PYTHON_VERSION})
target_link_libraries(
  python
  INTERFACE zlib wasi ${BINARY_DIR}/Modules/_hacl/libHacl_Hash_SHA2.a
            ${BINARY_DIR}/Modules/_decimal/libmpdec/libmpdec.a
            ${BINARY_DIR}/Modules/expat/libexpat.a)
