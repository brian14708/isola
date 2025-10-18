#include <catch2/catch_test_macros.hpp>
#include <promptkit.h>

#include <iostream>

void callback(promptkit_callback_event event, const uint8_t *data, size_t len,
              void *user_data) {
  if (event == PROMPTKIT_CALLBACK_EVENT_RESULT_JSON) {
    auto output = reinterpret_cast<std::vector<std::string> *>(user_data);
    output->push_back(std::string((const char *)data, len));
  } else if (event == PROMPTKIT_CALLBACK_EVENT_END_JSON) {
    auto output = reinterpret_cast<std::vector<std::string> *>(user_data);
    if (data != nullptr) {
      output->push_back(std::string((const char *)data, len));
    }
  }
}

TEST_CASE("Context") {
  promptkit_context_handle *ctx;
  REQUIRE(promptkit_context_create(0, &ctx) == 0);
  const char *path = std::getenv("PROMPTKIT_RUNTIME_PATH");
  REQUIRE(promptkit_context_initialize(ctx, path) == 0);
  promptkit_vm_handle *vm;
  REQUIRE(promptkit_vm_create(ctx, &vm) == 0);
  std::vector<std::string> outputs;
  REQUIRE(promptkit_vm_set_callback(vm, callback, &outputs) == 0);
  REQUIRE(promptkit_vm_start(vm) == 0);

  REQUIRE(promptkit_vm_load_script(
              vm, "def main():\n\tfor i in range(100): yield i", 1000) == 0);
  REQUIRE(promptkit_vm_run(vm, "main", nullptr, 0, 1000) == 0);

  REQUIRE(promptkit_vm_load_script(vm, "def main(i):\n\treturn i", 1000) == 0);
  promptkit_argument args[1];
  args[0].type = PROMPTKIT_ARGUMENT_TYPE_JSON;
  args[0].name = nullptr;
  args[0].value.data.data = reinterpret_cast<const uint8_t *>("100");
  args[0].value.data.len = 3;
  REQUIRE(promptkit_vm_run(vm, "main", args, 1, 1000) == 0);

  REQUIRE(outputs.size() == 101);
  for (size_t i = 0; i < outputs.size(); ++i) {
    REQUIRE(outputs[i] == std::to_string(i));
  }

  promptkit_vm_destroy(vm);
  promptkit_context_destroy(ctx);
}
