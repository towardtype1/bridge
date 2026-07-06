//! Hook payload parsing and hook response construction.
//!
//! Payload shape verified against live captures from CLI 2.1.201 (see
//! `tests/fixtures/hook_*.json`). Response schema per current docs:
//! `hookSpecificOutput.permissionDecision` for PreToolUse gating.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A parsed hook payload. Only fields the harness acts on are typed;
/// everything else is preserved in `extra` (schema tolerance + logging).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HookPayload {
    pub hook_event_name: String,
    pub session_id: String,
    pub cwd: PathBuf,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Present on Stop events.
    #[serde(default)]
    pub last_assistant_message: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl HookPayload {
    /// Parse from raw JSON. Errors only on malformed JSON or a missing
    /// `hook_event_name` / `session_id` / `cwd`; never panics. Unknown
    /// fields land in `extra`.
    pub fn parse(raw: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(raw.clone())
    }
}

/// `{}`: no opinion, let the normal permission flow decide. This is the
/// policy-pass response. NOT `allow`, which would bypass the static
/// `--allowedTools` gate (verified behavior, see plan ground truth item 7).
pub fn hook_response_passthrough() -> serde_json::Value {
    serde_json::json!({})
}

/// A deny decision with reason for the given hook event name. For events
/// that cannot deny (PostToolUse, Stop) this still renders the closest
/// blocking shape the schema offers (`decision: "block"`).
///
/// Schema per plan ground truth item 6 (2.1.x):
/// - PreToolUse: `hookSpecificOutput.permissionDecision = "deny"` with
///   `permissionDecisionReason`
/// - everything else: `{"decision": "block", "reason": ...}`
pub fn hook_response_deny(event: &str, reason: &str) -> serde_json::Value {
    if event == "PreToolUse" {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
    } else {
        serde_json::json!({"decision": "block", "reason": reason})
    }
}

/// An explicit allow. Reserved for user-approved escalations; bypasses the
/// remaining permission chain by design. Only PreToolUse has an allow
/// concept in the 2.1.x schema; for every other event this renders `{}`
/// (passthrough), which is the closest non-blocking response.
pub fn hook_response_allow(event: &str, reason: &str) -> serde_json::Value {
    if event == "PreToolUse" {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": reason,
            }
        })
    } else {
        hook_response_passthrough()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn passthrough_is_the_empty_object() {
        assert_eq!(hook_response_passthrough(), json!({}));
    }

    #[test]
    fn deny_pretooluse_uses_hook_specific_output() {
        assert_eq!(
            hook_response_deny("PreToolUse", "worktree escape"),
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "worktree escape"
                }
            })
        );
    }

    #[test]
    fn deny_posttooluse_and_stop_use_decision_block() {
        for event in ["PostToolUse", "Stop"] {
            assert_eq!(
                hook_response_deny(event, "not finished"),
                json!({"decision": "block", "reason": "not finished"}),
                "event: {event}"
            );
        }
    }

    #[test]
    fn deny_unknown_event_falls_back_to_the_blocking_shape() {
        assert_eq!(
            hook_response_deny("SubagentStop", "no"),
            json!({"decision": "block", "reason": "no"})
        );
    }

    #[test]
    fn allow_pretooluse_uses_hook_specific_output() {
        assert_eq!(
            hook_response_allow("PreToolUse", "user approved escalation"),
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": "user approved escalation"
                }
            })
        );
    }

    #[test]
    fn allow_on_events_without_an_allow_concept_is_passthrough() {
        for event in ["PostToolUse", "Stop", "SubagentStop"] {
            assert_eq!(
                hook_response_allow(event, "ok"),
                json!({}),
                "event: {event}"
            );
        }
    }
}
