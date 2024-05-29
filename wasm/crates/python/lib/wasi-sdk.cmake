include(FetchContent)

set(WASI_SDK_VERSION 22)
if(APPLE)
  set(WASI_HOST_OS "macos")
  set(WASI_SHA256
      "cf5f524de23f231756ec2f3754fc810ea3f6206841a968c45d8b7ea47cfc3a61")
else()
  set(WASI_HOST_OS "linux")
  set(WASI_SHA256
      "fa46b8f1b5170b0fecc0daf467c39f44a6d326b80ced383ec4586a50bc38d7b8")
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
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libc.a
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-signal.a
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-getpid.a
    ${WASMLIB_SYSROOT}/lib/wasm32-wasip1/libwasi-emulated-process-clocks.a
    ${WASI_SDK_PATH}/lib/clang/18/lib/wasip1/libclang_rt.builtins-wasm32.a)
