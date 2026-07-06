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
    /// `hook_event_name` / `session_id` / `cwd`; never panics.
    pub fn parse(_raw: &serde_json::Value) -> Result<Self, serde_json::Error> {
        todo!()
    }
}

/// `{}`: no opinion, let the normal permission flow decide. This is the
/// policy-pass response. NOT `allow`, which would bypass the static
/// `--allowedTools` gate (verified behavior, see plan ground truth item 7).
pub fn hook_response_passthrough() -> serde_json::Value {
    todo!()
}

/// A deny decision with reason for the given hook event name. For events
/// that cannot deny (PostToolUse, Stop) this still renders the closest
/// blocking shape the schema offers (`decision: "block"`).
pub fn hook_response_deny(_event: &str, _reason: &str) -> serde_json::Value {
    todo!()
}

/// An explicit allow. Reserved for user-approved escalations; bypasses the
/// remaining permission chain by design.
pub fn hook_response_allow(_event: &str, _reason: &str) -> serde_json::Value {
    todo!()
}
