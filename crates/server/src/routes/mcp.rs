use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars,
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::never::NeverSessionManager,
    },
};
use serde::Deserialize;
use serde_json::{json, value::RawValue};

use crate::routes::{AppState, ExecOptions, SandboxEnv, Source, StreamItem};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn server(state: AppState) -> StreamableHttpService<Sandbox, NeverSessionManager> {
    StreamableHttpService::new(
        move || Ok(Sandbox::new(state.clone())),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: false,
            ..Default::default()
        },
    )
}

#[derive(Clone)]
pub struct Sandbox {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
struct RunParams {
    python_code: String,
    #[serde(default)]
    #[schemars(description = "Timeout in seconds (default: 10)")]
    timeout_secs: Option<u64>,
}

#[tool_router]
impl Sandbox {
    fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(title = "Execute Python code in sandbox")]
    async fn run_python_code(
        &self,
        params: Parameters<RunParams>,
    ) -> Result<CallToolResult, McpError> {
        let timeout = params
            .0
            .timeout_secs
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);

        let exec_future = async {
            let mut stream = self
                .state
                .sandbox_manager
                .exec(
                    "mcp-trace",
                    Source {
                        prelude: String::new(),
                        code: params.0.python_code,
                    },
                    "main".to_string(),
                    vec![],
                    SandboxEnv {
                        client: self.state.base_env.client.clone(),
                    },
                    ExecOptions { timeout },
                )
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let mut results: Vec<serde_json::Value> = Vec::new();
            let mut final_result: Option<serde_json::Value> = None;
            let mut output = String::new();

            while let Some(item) = stream.next().await {
                match item {
                    StreamItem::Data(data) => {
                        let value = data
                            .to_json_value()
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        results.push(value);
                    }
                    StreamItem::End(Some(data)) => {
                        let value = data
                            .to_json_value()
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        final_result = Some(value);
                    }
                    StreamItem::End(None) => {}
                    StreamItem::Log { message, .. } => {
                        output.push_str(&message);
                    }
                    StreamItem::Error(err) => {
                        return Err(McpError::internal_error(err.to_string(), None));
                    }
                }
            }

            #[allow(clippy::option_if_let_else)]
            let result = if let Some(val) = final_result {
                val
            } else if results.len() == 1 {
                results.remove(0)
            } else {
                serde_json::Value::Array(results)
            };

            Ok::<(serde_json::Value, String), McpError>((result, output))
        };

        let (result, output) = match tokio::time::timeout(timeout, exec_future).await {
            Err(_) => {
                return Ok(CallToolResult::structured(json!({
                    "status": "error",
                    "message": "execution timeout exceeded",
                })));
            }
            Ok(Err(err)) => return Err(err),
            Ok(Ok(value)) => value,
        };

        let result_json = serde_json::to_string(&result)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut response = json!({
            "status": "success",
            "return": RawValue::from_string(result_json)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        });

        if !output.is_empty() {
            response["output"] = json!(output);
        }

        Ok(CallToolResult::structured(response))
    }
}

#[tool_handler]
impl ServerHandler for Sandbox {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                r#"
Sandboxed Python execution environment.

## Code Requirements
- Define entrypoint: `def main()` or `async def main()`
- Return value must be JSON-serializable
- `print()` output is captured
- Exceptions are captured as errors

## HTTP: sandbox.http

```python
from sandbox.http import fetch

async def main():
    async with fetch("GET", "https://api.example.com/data") as resp:
        data = await resp.ajson()  # or .atext() or .aread()
    return data
```

**Request options:**
- `body`: dict/list (JSON), bytes (raw), or use `files` for multipart
- `params`: dict for query params
- `headers`: dict for custom headers

**Response:**
- `resp.status`, `resp.headers`
- `await resp.ajson()`, `await resp.atext()`, `await resp.aread()`
- `async for line in resp.aiter_lines()`, `async for chunk in resp.aiter_bytes()`
- `async for event in resp.aiter_sse()` (SSE: `.id`, `.event`, `.data`)
                "#
                .to_string(),
            ),
        }
    }
}
