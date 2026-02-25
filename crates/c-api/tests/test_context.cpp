#include <catch2/catch_test_macros.hpp>
#include <isola.h>

#include <cstring>
#include <iostream>
#include <string>
#include <thread>
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

static std::string runtime_wasm_path() {
  const char *dir = std::getenv("ISOLA_RUNTIME_PATH");
  REQUIRE(dir != nullptr);
  return std::string(dir) + "/python3.wasm";
}

TEST_CASE("Context") {
  isola_context_handle *ctx;
  REQUIRE(isola_context_create(0, &ctx) == 0);
  auto path = runtime_wasm_path();
  REQUIRE(isola_context_initialize(ctx, path.c_str()) == 0);
  isola_sandbox_handle *sandbox;
  REQUIRE(isola_sandbox_create(ctx, &sandbox) == 0);
  callback_outputs outputs;
  isola_sandbox_handler_vtable vtable = {};
  vtable.on_event = callback;
  REQUIRE(isola_sandbox_set_handler(sandbox, &vtable, &outputs) == 0);
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

// ---------------------------------------------------------------------------
// HTTP mock handler test
// ---------------------------------------------------------------------------

struct http_test_context {
  callback_outputs outputs;
  std::string captured_method;
  std::string captured_url;
};

static isola_error_code mock_http_handler(const isola_http_request *request,
                                          isola_http_response_body *body,
                                          void *user_data) {
  auto *tc = reinterpret_cast<http_test_context *>(user_data);

  // Capture the request details for later assertions.
  tc->captured_method = request->method;
  tc->captured_url = request->url;

  // Deliver the response from a separate thread (non-blocking).
  std::thread([body]() {
    // Response headers
    std::string hdr_name = "x-mock";
    std::string hdr_value = "true";
    isola_http_header headers[1];
    headers[0].name = reinterpret_cast<const uint8_t *>(hdr_name.data());
    headers[0].name_len = hdr_name.size();
    headers[0].value = reinterpret_cast<const uint8_t *>(hdr_value.data());
    headers[0].value_len = hdr_value.size();

    isola_http_response_body_start(body, 200, headers, 1);

    // Push body in two chunks.
    std::string chunk1 = "hello ";
    std::string chunk2 = "from mock";
    isola_http_response_body_push(
        body, reinterpret_cast<const uint8_t *>(chunk1.data()), chunk1.size());
    isola_http_response_body_push(
        body, reinterpret_cast<const uint8_t *>(chunk2.data()), chunk2.size());

    isola_http_response_body_close(body);
  }).detach();

  return ISOLA_ERROR_CODE_OK;
}

static void mock_on_event(isola_callback_event event, const uint8_t *data,
                          size_t len, void *user_data) {
  auto *tc = reinterpret_cast<http_test_context *>(user_data);
  callback(event, data, len, &tc->outputs);
}

TEST_CASE("HTTP mock handler") {
  isola_context_handle *ctx;
  REQUIRE(isola_context_create(0, &ctx) == 0);
  auto path = runtime_wasm_path();
  REQUIRE(isola_context_initialize(ctx, path.c_str()) == 0);

  isola_sandbox_handle *sandbox;
  REQUIRE(isola_sandbox_create(ctx, &sandbox) == 0);

  http_test_context tc;
  isola_sandbox_handler_vtable vtable = {};
  vtable.on_event = mock_on_event;
  vtable.http_request = mock_http_handler;
  REQUIRE(isola_sandbox_set_handler(sandbox, &vtable, &tc) == 0);
  REQUIRE(isola_sandbox_start(sandbox) == 0);

  REQUIRE(isola_sandbox_load_script(
              sandbox,
              "from sandbox.http import fetch\n"
              "def main():\n"
              "    with fetch('GET', 'http://mock.test/hello') as resp:\n"
              "        return {'status': resp.status, 'body': resp.text()}\n",
              5000) == 0);

  REQUIRE(isola_sandbox_run(sandbox, "main", nullptr, 0, 5000) == 0);

  // The mock handler should have been called with the right request.
  REQUIRE(tc.captured_method == "GET");
  REQUIRE(tc.captured_url == "http://mock.test/hello");

  // The sandbox should have received a result with the mock body.
  REQUIRE(!tc.outputs.results.empty());
  auto &result = tc.outputs.results.back();
  REQUIRE(result.find("hello from mock") != std::string::npos);
  REQUIRE(result.find("200") != std::string::npos);

  isola_sandbox_destroy(sandbox);
  isola_context_destroy(ctx);
}
