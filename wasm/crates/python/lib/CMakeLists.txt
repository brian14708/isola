cmake_minimum_required(VERSION 3.25)
set(CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}")

# Common settings
set(WASMLIB_DOWNLOAD_DIR
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../downloads"
    CACHE PATH "Download directory")
set(WASMLIB_SYSROOT
    "${CMAKE_BINARY_DIR}/sysroot"
    CACHE PATH "Download directory")

include(wasi-sdk)
project(wasmlibs VERSION 1.0.0)

include(zlib)
include(python)
include(numpy/numpy)
