include(ExternalProject)

ExternalProject_Add(
  jpeg-build
  URL "https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/3.1.2/libjpeg-turbo-3.1.2.tar.gz"
  URL_HASH
    SHA256=8f0012234b464ce50890c490f18194f913a7b1f4e6a03d6644179fa0f867d0cf
  PREFIX _deps/jpeg
  DOWNLOAD_DIR ${WASMLIB_DOWNLOAD_DIR}
  INSTALL_DIR ${WASMLIB_SYSROOT}
  EXCLUDE_FROM_ALL TRUE
  CMAKE_ARGS -DCMAKE_BUILD_TYPE=Release
             -DCMAKE_TOOLCHAIN_FILE=${CMAKE_TOOLCHAIN_FILE}
             -DWITH_TURBOJPEG=OFF
             -DCMAKE_INSTALL_PREFIX=<INSTALL_DIR>
             -DENABLE_SHARED=OFF
             -DCMAKE_POSITION_INDEPENDENT_CODE=ON
             -DCMAKE_C_FLAGS=-mllvm\ -wasm-enable-sjlj
             -DCMAKE_EXE_LINKER_FLAGS=-lsetjmp)

add_library(jpeg STATIC IMPORTED GLOBAL)
add_dependencies(jpeg jpeg-build)
