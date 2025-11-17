use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::TryFutureExt;
use promptkit_trace::collect::CollectorSpanExt;
use promptkit_trace::consts::TRACE_TARGET_SCRIPT;
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
use tracing::{Instrument, Span, info_span};

use crate::proto::script::v1::ContentType;
use crate::proto::script::v1::result::ResultType;
use crate::proto::script::v1::trace::TraceType;
use crate::routes::{AppState, VmEnv};
use crate::service::non_stream_result;
use crate::utils::trace::TraceCollector;

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

        let (collector, rx) = TraceCollector::new();
        let s = info_span!(
            target: TRACE_TARGET_SCRIPT,
            parent: Span::current(),
            "script.exec"
        );
        let (span, mut trace, log_level) = if s
            .collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector)
            .is_some()
        {
            (s, Some(rx), LevelFilter::DEBUG)
        } else {
            (Span::none(), None, LevelFilter::OFF)
        };
        let exec_future = self
            .state
            .vm
            .exec(
                "",
                crate::routes::Source::Script {
                    prelude: String::new(),
                    code: params.0.python_code,
                },
                "main".to_string(),
                vec![],
                VmEnv {
                    client: self.state.base_env.client.clone(),
                    log_level,
                },
            )
            .instrument(span.clone());

        match tokio::time::timeout(timeout, exec_future).await {
            Err(_) => {
                return Ok(CallToolResult::structured(json!({
                    "status": "error",
                    "message": "execution timeout exceeded",
                })));
            }
            Ok(Err(e)) => {
                return Err(McpError::internal_error(e.to_string(), None));
            }
            Ok(Ok(mut r)) => {
                let result = non_stream_result(Pin::new(&mut r), [ContentType::Json as _])
                    .map_err(|e| McpError::internal_error(e.to_string(), None))
                    .instrument(span)
                    .await?;

                let mut output = String::new();
                if let Some(trace_events) = trace.as_mut() {
                    while let Some(event) = trace_events.recv().await {
                        if let Some(TraceType::Log(l)) = event.trace_type {
                            output.push_str(&l.content);
                        }
                    }
                }
                match result.result_type {
                    Some(ResultType::Json(v)) => {
                        let mut result = json!({
                            "status": "success",
                            "return": RawValue::from_string(v).map_err(|e| {
                                McpError::internal_error(e.to_string(), None)
                            })?,
                        });

                        // Only include output field if there's actual content
                        if !output.is_empty() {
                            result["output"] = json!(output);
                        }

                        return Ok(CallToolResult::structured(result));
                    }
                    Some(ResultType::Error(v)) => {
                        return Ok(CallToolResult::structured(json!({
                            "status": "error",
                            "message": v.message,
                        })));
                    }
                    _ => {
                        return Err(McpError::internal_error(
                            "Unexpected result type".to_string(),
                            None,
                        ));
                    }
                }
            }
        }
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

## HTTP: promptkit.http

```python
from promptkit.http import fetch

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
from promptkit.http import ws_connect

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
