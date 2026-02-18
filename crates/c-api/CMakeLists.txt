cmake_minimum_required(VERSION 3.12)
project(isola C)

include(ExternalProject)

if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(ISOLA_BUILD_TYPE "debug")
else()
  set(ISOLA_BUILD_TYPE_FLAG "--release")
  set(ISOLA_BUILD_TYPE "release")
endif()

set(ISOLA_TARGET_DIR
    ${CMAKE_CURRENT_SOURCE_DIR}/../../target/${ISOLA_BUILD_TYPE})

set(ISOLA_SHARED_LIB
    ${ISOLA_TARGET_DIR}/${CMAKE_SHARED_LIBRARY_PREFIX}isola${CMAKE_SHARED_LIBRARY_SUFFIX})

ExternalProject_Add(
  isola-c-api
  DOWNLOAD_COMMAND ""
  CONFIGURE_COMMAND ""
  INSTALL_COMMAND "${ISOLA_INSTALL_COMMAND}"
  BUILD_COMMAND
    cargo build --package isola-c-api-export ${ISOLA_BUILD_TYPE_FLAG}
    ${ISOLA_FEATURES} ${ISOLA_USER_CARGO_BUILD_OPTIONS}
  USES_TERMINAL_BUILD TRUE
  BINARY_DIR ${CMAKE_CURRENT_SOURCE_DIR}
  BUILD_ALWAYS ON
  BUILD_BYPRODUCTS ${ISOLA_SHARED_LIB})

add_library(isola INTERFACE)
add_dependencies(isola isola-c-api)
target_include_directories(isola
                           INTERFACE ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_link_libraries(isola INTERFACE ${ISOLA_SHARED_LIB})
