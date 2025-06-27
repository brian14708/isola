#include <catch2/catch_test_macros.hpp>
#include <promptkit.h>

TEST_CASE("Context") {
  promptkit_context_handle *ctx;
  promptkit_context_create(0, &ctx);
  const char *path = std::getenv("PROMPTKIT_RUNTIME_PATH");
  promptkit_context_initialize(ctx, path);
  promptkit_context_destroy(ctx);
}
