include(FetchContent)
include(ExternalProject)

set(PYTHON_VERSION 3.13)
FetchContent_Declare(
  python-src
  URL "https://www.python.org/ftp/python/3.13.5/Python-3.13.5.tar.xz"
  URL_HASH
    SHA256=93e583f243454e6e9e4588ca2c2662206ad961659863277afcdb96801647d640
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
    CFLAGS=-fPIC CONFIG_SITE=<SOURCE_DIR>/Tools/wasm/config.site-wasm32-wasi
    WASI_SDK_PATH=${WASI_SDK_PATH} <SOURCE_DIR>/Tools/wasm/wasi-env cmake -E env
    PKG_CONFIG_SYSROOT_DIR= -- <SOURCE_DIR>/configure --prefix=/usr/local
    --host=wasm32-wasi --enable-shared --build=${PYTHON_BUILD_ARCH}
    --with-build-python=${Python3_EXECUTABLE} --disable-test-modules
    --with-tzpath=/usr/local/lib/python3.13/site-packages/tzdata/zoneinfo
    --enable-big-digits=30
  PATCH_COMMAND patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/python.patch
  INSTALL_COMMAND DESTDIR=<INSTALL_DIR> make install
  BUILD_IN_SOURCE TRUE
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

file(
  GENERATE
  OUTPUT ${CMAKE_BINARY_DIR}/python-stub.c
  CONTENT "")

add_library(python SHARED ${CMAKE_BINARY_DIR}/python-stub.c)
set_target_properties(
  python
  PROPERTIES INTERFACE_INCLUDE_DIRECTORIES
             ${WASMLIB_SYSROOT}/usr/local/include/python${PYTHON_VERSION}
             OUTPUT_NAME python${PYTHON_VERSION})
add_dependencies(python python-build)
target_link_libraries(
  python
  PUBLIC -Wl,--whole-archive
         ${WASMLIB_SYSROOT}/usr/local/lib/libpython${PYTHON_VERSION}.a
         -Wl,--no-whole-archive)
target_link_libraries(
  python
  PRIVATE zlib wasi ${BINARY_DIR}/Modules/_hacl/libHacl_Hash_SHA2.a
          ${BINARY_DIR}/Modules/_decimal/libmpdec/libmpdec.a
          ${BINARY_DIR}/Modules/expat/libexpat.a)
install(TARGETS python DESTINATION lib)
install(FILES $<TARGET_PROPERTY:python,INTERFACE_INCLUDE_DIRECTORIES>
        DESTINATION include)
