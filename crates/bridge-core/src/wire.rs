//! Wire protocol between the hook helper binary and the control server.
//!
//! The helper forwards the raw hook JSON untouched; the server's response
//! body is the raw hook response JSON, printed verbatim to stdout by the
//! helper. Keeping both sides opaque here means schema evolution stays in
//! `bridge-compat`.

use crate::ids::WorkstreamId;
use serde::{Deserialize, Serialize};

/// Env var carrying the control server base URL, e.g. `http://127.0.0.1:49172`.
pub const ENV_SERVER_URL: &str = "BRIDGE_SERVER_URL";
/// Env var carrying the workstream id the hook belongs to.
pub const ENV_WORKSTREAM_ID: &str = "BRIDGE_WORKSTREAM_ID";
/// Env var carrying the per-workstream bearer token.
pub const ENV_TOKEN: &str = "BRIDGE_TOKEN";
/// Env var overriding the helper's HTTP timeout in milliseconds.
/// Default lives in the helper; keep it below the installed hook timeout.
pub const ENV_HELPER_TIMEOUT_MS: &str = "BRIDGE_HELPER_TIMEOUT_MS";

/// POST body for `POST /hook`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookWireRequest {
    pub workstream_id: WorkstreamId,
    /// Raw hook payload exactly as Claude Code wrote it to the helper's stdin.
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_request_round_trips() {
        let req = HookWireRequest {
            workstream_id: WorkstreamId::new(),
            payload: serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"}),
        };
        let body = serde_json::to_string(&req).unwrap();
        let back: HookWireRequest = serde_json::from_str(&body).unwrap();
        assert_eq!(req, back);
    }
}
