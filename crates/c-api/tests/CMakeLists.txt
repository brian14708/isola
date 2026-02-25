cmake_minimum_required(VERSION 3.25)
project(isola-tests-c LANGUAGES CXX)
set(CMAKE_C_STANDARD 17)

add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/..
                 ${CMAKE_BINARY_DIR}/c-api)

include(FetchContent)
FetchContent_Declare(
  Catch2 URL https://github.com/catchorg/Catch2/archive/refs/tags/v3.13.0.tar.gz)
FetchContent_MakeAvailable(Catch2)

include(CTest)
include(Catch)

set(COMMON_FLAGS -fsanitize=address,undefined)

add_executable(tests test_context.cpp)
target_compile_options(tests PRIVATE -Wpedantic -Werror ${COMMON_FLAGS} -g -O1
                                     -fno-omit-frame-pointer)
target_link_options(tests PRIVATE ${COMMON_FLAGS})
target_link_libraries(tests PRIVATE isola Catch2::Catch2WithMain)
catch_discover_tests(
  tests PROPERTIES ENVIRONMENT
  ISOLA_RUNTIME_PATH=${CMAKE_CURRENT_SOURCE_DIR}/../../../target)
