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
set(WASI_SDK_VERSION 27)
set(WASI_SHA256_x86_64-linux
    "b7d4d944c88503e4f21d84af07ac293e3440b1b6210bfd7fe78e0afd92c23bc2")
set(WASI_SHA256_arm64-linux
    "4cf4c553c4640e63e780442146f87d83fdff5737f988c06a6e3b2f0228e37665")
set(WASI_SHA256_arm64-macos
    "055c3dc2766772c38e71a05d353e35c322c7b2c6458a36a26a836f9808a550f8")

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
    ${WASI_SDK_PATH}/lib/clang/20/lib/wasm32-unknown-wasip2/libclang_rt.builtins.a
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libwasi-emulated-signal.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libwasi-emulated-process-clocks.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libwasi-emulated-getpid.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libdl.so
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libc.so)

install(
  FILES ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libc.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libdl.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libc++.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libc++abi.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libwasi-emulated-signal.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libwasi-emulated-getpid.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip2/libwasi-emulated-process-clocks.so
  DESTINATION lib)
