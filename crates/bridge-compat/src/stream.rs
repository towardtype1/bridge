//! stream-json NDJSON event parsing.
//!
//! One line in, one `StreamEvent` out. Schema-tolerant per the crate rules.
//! Shapes verified against fixtures captured from claude CLI 2.1.201; see
//! `tests/fixtures/README.md` for provenance.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StreamParseError {
    #[error("line is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("line is empty or whitespace")]
    Empty,
}

/// Parse one NDJSON line. Unknown `type` values yield `StreamEvent::Unknown`
/// (callers log and skip); only malformed JSON is an error. Never panics.
pub fn parse_line(_line: &str) -> Result<StreamEvent, StreamParseError> {
    todo!()
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// `{"type":"system","subtype":"init",...}` - first event of a run.
    SystemInit(SystemInit),
    /// `{"type":"system","subtype":"api_retry",...}` - retryable API error.
    ApiRetry(ApiRetry),
    /// Any other `system` subtype (e.g. `thinking_tokens`).
    SystemOther { subtype: String },
    /// `{"type":"assistant","message":{...}}` - an assistant API message.
    Assistant(AssistantEvent),
    /// `{"type":"user",...}` - tool results echoed back.
    UserToolResult(UserEvent),
    /// `{"type":"rate_limit_event",...}` - subscription rate limit status.
    RateLimit(RateLimitEvent),
    /// `{"type":"result",...}` - terminal event of a `-p` run.
    Result(ResultEvent),
    /// Unrecognized top-level `type`. Logged and skipped by callers.
    Unknown { raw_type: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemInit {
    pub session_id: String,
    pub model: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
    /// "none" when running on subscription auth (the required mode).
    #[serde(default)]
    pub api_key_source: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiRetry {
    pub attempt: u32,
    pub max_retries: Option<u32>,
    pub retry_delay_ms: Option<u64>,
    pub error_status: Option<u32>,
    /// Mapped from the `error` string; unknown strings become `Other`.
    pub error: ApiErrorCategory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorCategory {
    RateLimit,
    Overloaded,
    BillingError,
    AuthenticationFailed,
    ServerError,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssistantEvent {
    pub session_id: String,
    pub content: Vec<AssistantContent>,
    /// Model that produced the message (useful for fallback detection).
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssistantContent {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    Thinking,
    Other { block_type: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserEvent {
    pub session_id: String,
    /// (tool_use_id, is_error) pairs present in the message content.
    pub tool_results: Vec<(String, bool)>,
}

/// Snapshot of `rate_limit_info` from a `rate_limit_event`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitEvent {
    /// "allowed" when under the limit. Treat any other value as limited.
    pub status: String,
    /// Unix seconds when the limit window resets.
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub rate_limit_type: Option<String>,
}

/// Terminal `result` event. Field set verified 2.1.201; everything beyond
/// the always-present core is Option so error subtypes still parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultEvent {
    /// "success" or an error subtype such as "error_max_turns" or
    /// "error_during_execution". Preserved verbatim.
    pub subtype: String,
    pub is_error: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub num_turns: Option<u32>,
    /// Final assistant text.
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub api_error_status: Option<u32>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    /// Validated output when `--json-schema` was passed.
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    #[serde(default)]
    pub usage: Option<UsageTotals>,
    #[serde(default)]
    pub permission_denials: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
}

impl ResultEvent {
    /// Heuristic: does this result indicate the subscription limit was hit?
    /// True for rate-limit shaped error subtypes / api_error_status 429.
    pub fn looks_rate_limited(&self) -> bool {
        todo!()
    }
}
