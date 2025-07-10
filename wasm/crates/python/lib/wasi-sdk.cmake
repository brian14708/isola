include(FetchContent)

execute_process(
  COMMAND uname -m
  COMMAND tr -d '\n'
  OUTPUT_VARIABLE ARCHITECTURE)

if(CMAKE_HOST_SYSTEM_NAME STREQUAL "Linux")
  if(ARCHITECTURE MATCHES "aarch64")
    set(WASI_HOST_OS "arm64-linux")
  elseif(ARCHITECTURE MATCHES "x86_64")
    set(WASI_HOST_OS "x86_64-linux")
  else()
    message(
      FATAL_ERROR "Unsupported Linux arch: ${CMAKE_HOST_SYSTEM_PROCESSOR}")
  endif()
elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin")
  set(WASI_HOST_OS "arm64-macos")
else()
  message(FATAL_ERROR "Unsupported host OS: ${CMAKE_HOST_SYSTEM_NAME}")
endif()
set(WASI_SDK_VERSION 25)
set(WASI_SHA256_x86_64-linux
    "52640dde13599bf127a95499e61d6d640256119456d1af8897ab6725bcf3d89c")
set(WASI_SHA256_arm64-linux
    "47fccad8b2498f2239e05e1115c3ffc652bf37e7de2f88fb64b2d663c976ce2d")
set(WASI_SHA256_arm64-macos
    "e1e529ea226b1db0b430327809deae9246b580fa3cae32d31c82dfe770233587")

FetchContent_Declare(
  wasi-sdk
  URL "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/wasi-sdk-${WASI_SDK_VERSION}.0-${WASI_HOST_OS}.tar.gz"
  URL_HASH SHA256=${WASI_SHA256_${WASI_HOST_OS}}
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND
    ${CMAKE_COMMAND} -E rename <SOURCE_DIR>/share/wasi-sysroot
    ${WASMLIB_SYSROOT} && ${CMAKE_COMMAND} -E create_symlink ${WASMLIB_SYSROOT}
    <SOURCE_DIR>/share/wasi-sysroot)
FetchContent_MakeAvailable(wasi-sdk)

message(${wasi-sdk_SOURCE_DIR})
set(CMAKE_TOOLCHAIN_FILE ${wasi-sdk_SOURCE_DIR}/share/cmake/wasi-sdk-p1.cmake)
set(WASI_SDK_PATH ${wasi-sdk_SOURCE_DIR})

add_library(wasi INTERFACE)
target_link_libraries(
  wasi
  INTERFACE
    ${WASI_SDK_PATH}/lib/clang/19/lib/wasip1/libclang_rt.builtins-wasm32.a
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-signal.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-process-clocks.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-getpid.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libdl.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc.so)

install(
  FILES ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libdl.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc++.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc++abi.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-signal.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-getpid.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-process-clocks.so
  DESTINATION lib)
