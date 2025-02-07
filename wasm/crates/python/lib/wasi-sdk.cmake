include(FetchContent)

set(WASI_SDK_VERSION 24)
if(APPLE)
  set(WASI_HOST_OS "arm64-macos")
  set(WASI_SHA256
      "aeae999396d5f5caa5ce419f52e83c35869d5fd21d40af80acba2c80f51b0b3a")
else()
  set(WASI_HOST_OS "x86_64-linux")
  set(WASI_SHA256
      "c6c38aab56e5de88adf6c1ebc9c3ae8da72f88ec2b656fb024eda8d4167a0bc5")
endif()

FetchContent_Declare(
  wasi-sdk
  URL "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/wasi-sdk-${WASI_SDK_VERSION}.0-${WASI_HOST_OS}.tar.gz"
  URL_HASH SHA256=${WASI_SHA256}
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  PATCH_COMMAND
    ${CMAKE_COMMAND} -E rename <SOURCE_DIR>/share/wasi-sysroot
    ${WASMLIB_SYSROOT} && ${CMAKE_COMMAND} -E create_symlink ${WASMLIB_SYSROOT}
    <SOURCE_DIR>/share/wasi-sysroot)
FetchContent_MakeAvailable(wasi-sdk)

message(${wasi-sdk_SOURCE_DIR})
set(CMAKE_TOOLCHAIN_FILE ${wasi-sdk_SOURCE_DIR}/share/cmake/wasi-sdk.cmake)
set(WASI_SDK_PATH ${wasi-sdk_SOURCE_DIR})

add_library(wasi INTERFACE)
target_link_libraries(
  wasi
  INTERFACE
    ${WASI_SDK_PATH}/lib/clang/18/lib/wasip1/libclang_rt.builtins-wasm32.a)

install(
  FILES ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libdl.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc++.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc++abi.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-signal.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-getpid.so
        ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-process-clocks.so
  DESTINATION lib)
