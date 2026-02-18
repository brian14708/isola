use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use isola::TRACE_TARGET_SCRIPT;
use isola_trace::collect::CollectSpanExt;
use rmcp::schemars;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::never::NeverSessionManager,
    },
};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use tracing::level_filters::LevelFilter;
use tracing::{Span, info_span};

use crate::routes::api::trace::{HttpTraceCollector, TraceData};
use crate::routes::{AppState, Source, StreamItem, VmEnv};

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

        let (collector, rx) = HttpTraceCollector::new();
        let s = info_span!(
            target: TRACE_TARGET_SCRIPT,
            parent: Span::current(),
            "script.exec"
        );
        let (span, mut trace_rx, log_level) = if s
            .collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector)
            .is_some()
        {
            (s, Some(rx), LevelFilter::DEBUG)
        } else {
            (Span::none(), None, LevelFilter::OFF)
        };

        let exec_future = async {
            let _enter = span.enter();
            let mut stream = self
                .state
                .vm
                .exec(
                    "mcp-trace",
                    Source {
                        prelude: String::new(),
                        code: params.0.python_code,
                    },
                    "main".to_string(),
                    vec![],
                    timeout,
                    VmEnv {
                        client: self.state.base_env.client.clone(),
                        log_level,
                    },
                )
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let mut results: Vec<serde_json::Value> = Vec::new();
            let mut final_result: Option<serde_json::Value> = None;

            while let Some(item) = stream.next().await {
                match item {
                    StreamItem::Data(data) => {
                        let json_str = isola_cbor::cbor_to_json(&data)
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        let value: serde_json::Value = serde_json::from_str(&json_str)
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        results.push(value);
                    }
                    StreamItem::End(Some(data)) => {
                        let json_str = isola_cbor::cbor_to_json(&data)
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        let value: serde_json::Value = serde_json::from_str(&json_str)
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                        final_result = Some(value);
                    }
                    StreamItem::End(None) => {}
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

            Ok::<serde_json::Value, McpError>(result)
        };

        let result = match tokio::time::timeout(timeout, exec_future).await {
            Err(_) => {
                return Ok(CallToolResult::structured(json!({
                    "status": "error",
                    "message": "execution timeout exceeded",
                })));
            }
            Ok(Err(err)) => return Err(err),
            Ok(Ok(value)) => value,
        };

        let mut output = String::new();
        if let Some(ref mut trace_events) = trace_rx {
            while let Ok(event) = trace_events.try_recv() {
                if let TraceData::Log { message, .. } = event.data {
                    output.push_str(&message);
                }
            }
        }

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

**WebSocket:**
```python
from sandbox.http import ws_connect

async def main():
    async with ws_connect("wss://example.com/ws") as ws:
        await ws.asend("Hello")
        return await ws.arecv()
```
                "#
                .to_string(),
            ),
        }
    }
}
