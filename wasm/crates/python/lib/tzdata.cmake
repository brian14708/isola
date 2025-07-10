include(FetchContent)
include(ExternalProject)

FetchContent_Declare(
  tzdata-src
  URL "https://files.pythonhosted.org/packages/5c/23/c7abc0ca0a1526a0774eca151daeb8de62ec457e77262b66b359c3c7679e/tzdata-2025.2-py2.py3-none-any.whl"
  URL_HASH
    SHA256=1a403fada01ff9221ca8044d701868fa132215d84beb92242d9acd2147f667a8
  DOWNLOAD_NO_EXTRACT ON
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR})
FetchContent_MakeAvailable(tzdata-src)

ExternalProject_Add(
  tzdata-build
  PREFIX _deps/tzdata
  SOURCE_DIR ${tzdata-src_SOURCE_DIR}
  BUILD_IN_SOURCE TRUE
  CONFIGURE_COMMAND ""
  BUILD_COMMAND ""
  INSTALL_COMMAND
    cmake -E make_directory
    ${CMAKE_BINARY_DIR}/pythonpkgs/lib/python3.13/site-packages && cd
    ${CMAKE_BINARY_DIR}/pythonpkgs/lib/python3.13/site-packages &&
    ${CMAKE_COMMAND} -E tar xvf
    ${tzdata-src_SOURCE_DIR}/tzdata-2025.2-py2.py3-none-any.whl)
