cmake_minimum_required(VERSION 3.12)
project(promptkit C)

include(ExternalProject)

if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(PROMPTKIT_BUILD_TYPE "debug")
else()
  set(PROMPTKIT_BUILD_TYPE_FLAG "--release")
  set(PROMPTKIT_BUILD_TYPE "release")
endif()

set(PROMPTKIT_TARGET_DIR
    ${CMAKE_CURRENT_SOURCE_DIR}/../../target/${PROMPTKIT_BUILD_TYPE})

set(PROMPTKIT_SHARED_LIB ${PROMPTKIT_TARGET_DIR}/libpromptkit.so)

ExternalProject_Add(
  promptkit-c-api
  DOWNLOAD_COMMAND ""
  CONFIGURE_COMMAND ""
  INSTALL_COMMAND "${PROMPTKIT_INSTALL_COMMAND}"
  BUILD_COMMAND
    cargo build --package promptkit-c-api ${PROMPTKIT_BUILD_TYPE_FLAG}
    ${PROMPTKIT_FEATURES} ${PROMPTKIT_USER_CARGO_BUILD_OPTIONS}
  USES_TERMINAL_BUILD TRUE
  BINARY_DIR ${CMAKE_CURRENT_SOURCE_DIR}
  BUILD_ALWAYS ON
  BUILD_BYPRODUCTS ${PROMPTKIT_SHARED_LIB})

add_library(promptkit INTERFACE)
add_dependencies(promptkit promptkit-c-api)
target_include_directories(promptkit
                           INTERFACE ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_link_libraries(promptkit INTERFACE ${PROMPTKIT_SHARED_LIB})
