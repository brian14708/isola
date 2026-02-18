use serde::{Deserialize, Serialize};

use super::trace::HttpTrace;

const fn default_timeout() -> u64 {
    30000
}

fn default_function() -> String {
    "main".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    pub runtime: String,
    pub script: String,
    #[serde(default)]
    pub prelude: String,
    #[serde(default = "default_function")]
    pub function: String,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
    #[serde(default)]
    pub kwargs: serde_json::Map<String, serde_json::Value>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub trace: bool,
}

#[derive(Debug, Serialize)]
pub struct ExecuteResponse {
    pub result: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<HttpTrace>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: HttpError,
}

#[derive(Debug, Serialize)]
pub struct HttpError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidRequest,
    UnknownRuntime,
    ScriptError,
    Timeout,
    Cancelled,
    Internal,
}

#[derive(Debug, Serialize)]
pub struct SseDataEvent {
    pub value: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SseDoneEvent {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<HttpTrace>,
}

#[derive(Debug, Serialize)]
pub struct SseErrorEvent {
    pub code: ErrorCode,
    pub message: String,
}
