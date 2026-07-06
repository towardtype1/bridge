//! stream-json NDJSON event parsing.
//!
//! One line in, one `StreamEvent` out. Schema-tolerant per the crate rules.
//! Shapes verified against fixtures captured from claude CLI 2.1.201; see
//! `tests/fixtures/README.md` for provenance.

use serde::{Deserialize, Serialize};
use serde_json::Value;
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
///
/// Tolerance details:
/// - a line that is valid JSON but not an object with a string `type` field
///   parses to `Unknown { raw_type: "" }`
/// - unknown fields anywhere are ignored
/// - a KNOWN event type missing a required field (e.g. a `result` without
///   `subtype`) is a `StreamParseError::Json` error, since silently dropping
///   a terminal event would mask real breakage
pub fn parse_line(line: &str) -> Result<StreamEvent, StreamParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(StreamParseError::Empty);
    }
    let value: Value = serde_json::from_str(trimmed)?;
    let Some(raw_type) = value.get("type").and_then(Value::as_str) else {
        tracing::debug!("stream line has no string `type` field; skipping as Unknown");
        return Ok(StreamEvent::Unknown {
            raw_type: String::new(),
        });
    };
    match raw_type {
        "system" => parse_system(&value),
        "assistant" => parse_assistant(&value),
        "user" => parse_user(&value),
        "rate_limit_event" => parse_rate_limit(&value),
        "result" => Ok(StreamEvent::Result(serde_json::from_value(value)?)),
        other => {
            tracing::debug!(raw_type = other, "unknown stream event type; skipping");
            Ok(StreamEvent::Unknown {
                raw_type: other.to_owned(),
            })
        }
    }
}

fn parse_system(value: &Value) -> Result<StreamEvent, StreamParseError> {
    let subtype = value
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match subtype {
        "init" => Ok(StreamEvent::SystemInit(serde_json::from_value(
            value.clone(),
        )?)),
        "api_retry" => Ok(StreamEvent::ApiRetry(parse_api_retry(value))),
        other => Ok(StreamEvent::SystemOther {
            subtype: other.to_owned(),
        }),
    }
}

fn parse_api_retry(value: &Value) -> ApiRetry {
    let as_u32 = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    ApiRetry {
        attempt: as_u32("attempt").unwrap_or(0),
        max_retries: as_u32("max_retries"),
        retry_delay_ms: value.get("retry_delay_ms").and_then(Value::as_u64),
        error_status: as_u32("error_status"),
        error: value
            .get("error")
            .and_then(Value::as_str)
            .map_or(ApiErrorCategory::Other(String::new()), error_category),
    }
}

/// Map the documented `system/api_retry` error category strings; unknown
/// strings are preserved verbatim in `Other`.
fn error_category(raw: &str) -> ApiErrorCategory {
    match raw {
        "rate_limit" => ApiErrorCategory::RateLimit,
        "overloaded" => ApiErrorCategory::Overloaded,
        "billing_error" => ApiErrorCategory::BillingError,
        "authentication_failed" => ApiErrorCategory::AuthenticationFailed,
        "server_error" => ApiErrorCategory::ServerError,
        other => ApiErrorCategory::Other(other.to_owned()),
    }
}

/// Extract a required top-level string field, producing a `serde_json`
/// error (not a panic) when it is absent.
fn require_str(value: &Value, field: &str) -> Result<String, StreamParseError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            StreamParseError::Json(<serde_json::Error as serde::de::Error>::custom(format!(
                "missing required string field `{field}`"
            )))
        })
}

fn parse_assistant(value: &Value) -> Result<StreamEvent, StreamParseError> {
    let session_id = require_str(value, "session_id")?;
    let message = value.get("message");
    let model = message
        .and_then(|m| m.get("model"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let content = message
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().map(parse_content_block).collect())
        .unwrap_or_default();
    Ok(StreamEvent::Assistant(AssistantEvent {
        session_id,
        content,
        model,
    }))
}

fn parse_content_block(block: &Value) -> AssistantContent {
    let str_field = |field: &str| {
        block
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => AssistantContent::Text {
            text: str_field("text"),
        },
        "thinking" => AssistantContent::Thinking,
        "tool_use" => AssistantContent::ToolUse {
            id: str_field("id"),
            name: str_field("name"),
            input: block.get("input").cloned().unwrap_or(Value::Null),
        },
        other => AssistantContent::Other {
            block_type: other.to_owned(),
        },
    }
}

fn parse_user(value: &Value) -> Result<StreamEvent, StreamParseError> {
    let session_id = require_str(value, "session_id")?;
    let tool_results = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(Value::as_str) != Some("tool_result") {
                        return None;
                    }
                    let id = item.get("tool_use_id").and_then(Value::as_str)?.to_owned();
                    let is_error = item
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some((id, is_error))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(StreamEvent::UserToolResult(UserEvent {
        session_id,
        tool_results,
    }))
}

fn parse_rate_limit(value: &Value) -> Result<StreamEvent, StreamParseError> {
    let info = value.get("rate_limit_info").ok_or_else(|| {
        StreamParseError::Json(<serde_json::Error as serde::de::Error>::custom(
            "rate_limit_event missing `rate_limit_info`",
        ))
    })?;
    Ok(StreamEvent::RateLimit(serde_json::from_value(
        info.clone(),
    )?))
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
    /// Wire key is camelCase `apiKeySource` (verified 2.1.201).
    #[serde(default, alias = "apiKeySource")]
    pub api_key_source: Option<String>,
    /// Wire key is camelCase `permissionMode` (verified 2.1.201).
    #[serde(default, alias = "permissionMode")]
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
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Thinking,
    Other {
        block_type: String,
    },
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
    /// Wire key is camelCase `resetsAt` (verified 2.1.201).
    #[serde(default, alias = "resetsAt")]
    pub resets_at: Option<i64>,
    /// Wire key is camelCase `rateLimitType` (verified 2.1.201).
    #[serde(default, alias = "rateLimitType")]
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
    ///
    /// Deliberately conservative; true only when:
    /// - `subtype` contains `rate_limit` (case-insensitive), or
    /// - `api_error_status` is exactly 429, or
    /// - `is_error` AND `stop_reason` or `terminal_reason` mentions
    ///   `rate_limit`, `rate limit` or `throttl` (case-insensitive)
    ///
    /// Overload (529) and other server errors are NOT treated as rate
    /// limits: they are retried by the CLI itself, not paused on.
    pub fn looks_rate_limited(&self) -> bool {
        if self.subtype.to_ascii_lowercase().contains("rate_limit") {
            return true;
        }
        if self.api_error_status == Some(429) {
            return true;
        }
        if self.is_error {
            let throttled = |reason: Option<&str>| {
                reason.is_some_and(|r| {
                    let r = r.to_ascii_lowercase();
                    r.contains("rate_limit") || r.contains("rate limit") || r.contains("throttl")
                })
            };
            if throttled(self.stop_reason.as_deref()) || throttled(self.terminal_reason.as_deref())
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn unknown_top_level_type_parses_to_unknown() {
        let event = parse_line(r#"{"type":"subspace_anomaly","x":1}"#).unwrap();
        assert_eq!(
            event,
            StreamEvent::Unknown {
                raw_type: "subspace_anomaly".into()
            }
        );
    }

    #[test]
    fn unknown_system_subtype_parses_to_system_other() {
        let event =
            parse_line(r#"{"type":"system","subtype":"warp_core_status","level":9}"#).unwrap();
        assert_eq!(
            event,
            StreamEvent::SystemOther {
                subtype: "warp_core_status".into()
            }
        );
    }

    #[test]
    fn junk_line_is_an_error_not_a_panic() {
        assert!(matches!(
            parse_line("{not json at all"),
            Err(StreamParseError::Json(_))
        ));
        assert!(matches!(
            parse_line("plain text line"),
            Err(StreamParseError::Json(_))
        ));
    }

    #[test]
    fn empty_or_whitespace_line_is_the_empty_error() {
        assert!(matches!(parse_line(""), Err(StreamParseError::Empty)));
        assert!(matches!(parse_line("   \t "), Err(StreamParseError::Empty)));
    }

    #[test]
    fn valid_json_without_a_type_string_is_unknown_with_empty_raw_type() {
        for line in [
            r#"{"notype":1}"#,
            "42",
            r#""hello""#,
            "[1,2]",
            r#"{"type":7}"#,
        ] {
            let event = parse_line(line).unwrap();
            assert_eq!(
                event,
                StreamEvent::Unknown {
                    raw_type: String::new()
                },
                "line: {line}"
            );
        }
    }

    #[test]
    fn extra_unknown_fields_are_ignored_everywhere() {
        let init = parse_line(
            r#"{"type":"system","subtype":"init","session_id":"s","model":"m","warp_factor":9,"future":{"x":1}}"#,
        )
        .unwrap();
        let StreamEvent::SystemInit(init) = init else {
            panic!("expected SystemInit, got {init:?}");
        };
        assert_eq!(init.session_id, "s");
        assert_eq!(init.model, "m");
        assert!(init.tools.is_empty());
        assert_eq!(init.claude_code_version, None);

        let result =
            parse_line(r#"{"type":"result","subtype":"success","is_error":false,"warp_factor":9}"#)
                .unwrap();
        let StreamEvent::Result(result) = result else {
            panic!("expected Result, got {result:?}");
        };
        assert_eq!(result.subtype, "success");
        assert_eq!(result.usage, None);
    }

    #[test]
    fn unknown_assistant_content_block_type_becomes_other() {
        let event = parse_line(
            r#"{"type":"assistant","session_id":"s","message":{"content":[{"type":"server_tool_use","x":1}]}}"#,
        )
        .unwrap();
        let StreamEvent::Assistant(a) = event else {
            panic!("expected Assistant");
        };
        assert_eq!(
            a.content,
            vec![AssistantContent::Other {
                block_type: "server_tool_use".into()
            }]
        );
        assert_eq!(a.model, None);
    }

    #[test]
    fn known_event_missing_required_fields_is_an_error() {
        // result without subtype / is_error
        assert!(parse_line(r#"{"type":"result","is_error":false}"#).is_err());
        // assistant without session_id
        assert!(parse_line(r#"{"type":"assistant","message":{"content":[]}}"#).is_err());
        // user without session_id
        assert!(parse_line(r#"{"type":"user","message":{"content":[]}}"#).is_err());
        // rate_limit_event without rate_limit_info
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_err());
    }

    #[test]
    fn api_retry_error_strings_map_to_categories() {
        let cases = [
            ("rate_limit", ApiErrorCategory::RateLimit),
            ("overloaded", ApiErrorCategory::Overloaded),
            ("billing_error", ApiErrorCategory::BillingError),
            (
                "authentication_failed",
                ApiErrorCategory::AuthenticationFailed,
            ),
            ("server_error", ApiErrorCategory::ServerError),
            (
                "romulan_jamming",
                ApiErrorCategory::Other("romulan_jamming".into()),
            ),
        ];
        for (raw, expected) in cases {
            let line =
                format!(r#"{{"type":"system","subtype":"api_retry","attempt":1,"error":"{raw}"}}"#);
            let StreamEvent::ApiRetry(retry) = parse_line(&line).unwrap() else {
                panic!("expected ApiRetry for {raw}");
            };
            assert_eq!(retry.error, expected, "category for {raw}");
            assert_eq!(retry.attempt, 1);
            assert_eq!(retry.max_retries, None);
        }
    }

    fn result_from(v: serde_json::Value) -> ResultEvent {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn looks_rate_limited_true_cases() {
        // Subtype mentions rate_limit.
        assert!(
            result_from(serde_json::json!({"subtype": "error_rate_limit", "is_error": true}))
                .looks_rate_limited()
        );
        // HTTP 429 regardless of subtype.
        assert!(result_from(
            serde_json::json!({"subtype": "error_during_execution", "is_error": true, "api_error_status": 429})
        )
        .looks_rate_limited());
        // Error whose stop/terminal reason indicates throttling.
        assert!(result_from(
            serde_json::json!({"subtype": "error_during_execution", "is_error": true, "stop_reason": "rate_limited"})
        )
        .looks_rate_limited());
        assert!(result_from(
            serde_json::json!({"subtype": "error_during_execution", "is_error": true, "terminal_reason": "throttled"})
        )
        .looks_rate_limited());
    }

    #[test]
    fn looks_rate_limited_stays_conservative() {
        // Plain success.
        assert!(
            !result_from(serde_json::json!({"subtype": "success", "is_error": false}))
                .looks_rate_limited()
        );
        // Ordinary errors.
        assert!(
            !result_from(serde_json::json!({"subtype": "error_max_turns", "is_error": true}))
                .looks_rate_limited()
        );
        assert!(!result_from(
            serde_json::json!({"subtype": "error_during_execution", "is_error": true, "stop_reason": "end_turn"})
        )
        .looks_rate_limited());
        // Overload (529) is not a subscription rate limit.
        assert!(!result_from(
            serde_json::json!({"subtype": "error_during_execution", "is_error": true, "api_error_status": 529})
        )
        .looks_rate_limited());
        // Throttle-flavored reason on a NON-error result does not trigger.
        assert!(!result_from(
            serde_json::json!({"subtype": "success", "is_error": false, "stop_reason": "rate_limited"})
        )
        .looks_rate_limited());
    }

    proptest! {
        #[test]
        fn parse_line_never_panics(line in ".*") {
            let _ = parse_line(&line);
        }

        #[test]
        fn arbitrary_type_names_parse_to_unknown(t in "[a-z_]{1,20}") {
            prop_assume!(!matches!(
                t.as_str(),
                "system" | "assistant" | "user" | "rate_limit_event" | "result"
            ));
            let line = serde_json::json!({"type": &t}).to_string();
            let event = parse_line(&line).unwrap();
            prop_assert_eq!(event, StreamEvent::Unknown { raw_type: t });
        }
    }
}
