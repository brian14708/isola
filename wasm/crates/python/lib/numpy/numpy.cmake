include(FetchContent)
include(ExternalProject)

find_package(Python3 ${PYTHON_VERSION})

FetchContent_Declare(
  numpy-src
  URL "https://github.com/numpy/numpy/releases/download/v2.3.4/numpy-2.3.4.tar.gz"
  URL_HASH
    SHA256=a7d018bfedb375a8d979ac758b120ba846a7fe764911a64465fd87b8729f4a6a
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND patch -p1 < ${CMAKE_CURRENT_LIST_DIR}/patch.diff)
FetchContent_MakeAvailable(numpy-src)

configure_file(${CMAKE_CURRENT_LIST_DIR}/cross.cfg.in
               ${numpy-src_SOURCE_DIR}/cross.cfg @ONLY)

ExternalProject_Add(
  numpy-build
  PREFIX _deps/numpy
  SOURCE_DIR ${numpy-src_SOURCE_DIR}
  CONFIGURE_COMMAND
    PYTHONPATH=${WASMLIB_SYSROOT}/usr/local/lib/python3.14
    _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
    ${Python3_EXECUTABLE} <SOURCE_DIR>/vendored-meson/meson/meson.py setup
    --prefix ${CMAKE_BINARY_DIR}/pythonpkgs
    --cross-file=${numpy-src_SOURCE_DIR}/cross.cfg --buildtype release
    <BINARY_DIR> <SOURCE_DIR>
  BUILD_COMMAND ninja
  INSTALL_COMMAND
    ${Python3_EXECUTABLE} <SOURCE_DIR>/vendored-meson/meson/meson.py install
    --no-rebuild --tags=runtime,python-runtime,devel
  DEPENDS python python-stub)

install(DIRECTORY ${CMAKE_BINARY_DIR}/pythonpkgs/ DESTINATION usr/local)
