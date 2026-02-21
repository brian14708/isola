#include <catch2/catch_test_macros.hpp>
#include <isola.h>

#include <iostream>
#include <string>
#include <vector>

struct callback_outputs {
  std::vector<std::string> results;
  std::vector<std::string> stdout;
  std::vector<std::string> logs;
};

void callback(isola_callback_event event, const uint8_t *data, size_t len,
              void *user_data) {
  auto output = reinterpret_cast<callback_outputs *>(user_data);
  if (event == ISOLA_CALLBACK_EVENT_RESULT_JSON) {
    output->results.push_back(std::string((const char *)data, len));
  } else if (event == ISOLA_CALLBACK_EVENT_END_JSON) {
    if (data != nullptr) {
      output->results.push_back(std::string((const char *)data, len));
    }
  } else if (event == ISOLA_CALLBACK_EVENT_STDOUT) {
    output->stdout.push_back(std::string((const char *)data, len));
  } else if (event == ISOLA_CALLBACK_EVENT_LOG) {
    output->logs.push_back(std::string((const char *)data, len));
  }
}

TEST_CASE("Context") {
  isola_context_handle *ctx;
  REQUIRE(isola_context_create(0, &ctx) == 0);
  const char *path = std::getenv("ISOLA_RUNTIME_PATH");
  REQUIRE(isola_context_initialize(ctx, path) == 0);
  isola_sandbox_handle *sandbox;
  REQUIRE(isola_sandbox_create(ctx, &sandbox) == 0);
  callback_outputs outputs;
  REQUIRE(isola_sandbox_set_callback(sandbox, callback, &outputs) == 0);
  REQUIRE(isola_sandbox_start(sandbox) == 0);

  REQUIRE(isola_sandbox_load_script(
              sandbox, "def main():\n\tfor i in range(100): yield i", 1000) ==
          0);
  REQUIRE(isola_sandbox_run(sandbox, "main", nullptr, 0, 1000) == 0);

  REQUIRE(isola_sandbox_load_script(sandbox, "def main(i):\n\treturn i",
                                    1000) == 0);
  isola_argument args[1];
  args[0].type = ISOLA_ARGUMENT_TYPE_JSON;
  args[0].name = nullptr;
  args[0].value.data.data = reinterpret_cast<const uint8_t *>("100");
  args[0].value.data.len = 3;
  REQUIRE(isola_sandbox_run(sandbox, "main", args, 1, 1000) == 0);

  REQUIRE(outputs.results.size() == 101);
  for (size_t i = 0; i < outputs.results.size(); ++i) {
    REQUIRE(outputs.results[i] == std::to_string(i));
  }

  REQUIRE(isola_sandbox_load_script(sandbox,
                                    "import sandbox.logging\n"
                                    "def main():\n"
                                    "\tprint('hello-stdout')\n"
                                    "\tsandbox.logging.info('hello-log')\n"
                                    "\treturn 101",
                                    1000) == 0);
  REQUIRE(isola_sandbox_run(sandbox, "main", nullptr, 0, 1000) == 0);

  REQUIRE(outputs.results.size() == 102);
  REQUIRE(outputs.results[101] == "101");
  REQUIRE(!outputs.stdout.empty());
  REQUIRE(outputs.stdout[0].find("hello-stdout") != std::string::npos);
  REQUIRE(!outputs.logs.empty());
  REQUIRE(outputs.logs[0].find("hello-log") != std::string::npos);

  isola_sandbox_destroy(sandbox);
  isola_context_destroy(ctx);
}
