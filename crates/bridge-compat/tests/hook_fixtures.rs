//! Hook payload parsing against real 2.1.201 captures.

use bridge_compat::HookPayload;
use std::path::PathBuf;

fn fixture(name: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} is not JSON: {e}"))
}

const SESSION: &str = "a2356b0f-c205-48cc-8654-88c062868484";

#[test]
fn pretooluse_payload_parses_typed_fields_and_keeps_extras() {
    let payload = HookPayload::parse(&fixture("hook_pretooluse.json")).expect("parses");
    assert_eq!(payload.hook_event_name, "PreToolUse");
    assert_eq!(payload.session_id, SESSION);
    assert_eq!(payload.cwd, PathBuf::from("/tmp/lab"));
    assert_eq!(payload.permission_mode.as_deref(), Some("default"));
    assert_eq!(payload.tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        payload.tool_input,
        Some(serde_json::json!({
            "command": "echo LCARS-OK",
            "description": "Run the specified echo command"
        }))
    );
    assert_eq!(
        payload.tool_use_id.as_deref(),
        Some("toolu_01E4aMtwwd8YYvxiCB2PB7WD")
    );
    assert_eq!(payload.last_assistant_message, None);
    // Untyped fields survive in `extra` for logging/deep-scan.
    assert!(payload.extra.contains_key("transcript_path"));
    assert!(payload.extra.contains_key("prompt_id"));
    // Typed fields are NOT duplicated into extra.
    assert!(!payload.extra.contains_key("tool_name"));
    assert!(!payload.extra.contains_key("session_id"));
}

#[test]
fn posttooluse_payload_parses_and_keeps_tool_response_in_extra() {
    let payload = HookPayload::parse(&fixture("hook_posttooluse.json")).expect("parses");
    assert_eq!(payload.hook_event_name, "PostToolUse");
    assert_eq!(payload.tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        payload.tool_use_id.as_deref(),
        Some("toolu_01E4aMtwwd8YYvxiCB2PB7WD")
    );
    let tool_response = payload
        .extra
        .get("tool_response")
        .expect("tool_response kept in extra");
    assert_eq!(tool_response["stdout"], "LCARS-OK");
    assert_eq!(
        payload.extra.get("duration_ms"),
        Some(&serde_json::json!(367))
    );
}

#[test]
fn stop_payload_parses_last_assistant_message() {
    let payload = HookPayload::parse(&fixture("hook_stop.json")).expect("parses");
    assert_eq!(payload.hook_event_name, "Stop");
    assert_eq!(payload.session_id, SESSION);
    assert_eq!(payload.last_assistant_message.as_deref(), Some("DONE"));
    assert_eq!(payload.tool_name, None);
    assert_eq!(payload.tool_input, None);
    assert_eq!(payload.tool_use_id, None);
    assert_eq!(
        payload.extra.get("stop_hook_active"),
        Some(&serde_json::json!(false))
    );
    assert!(payload.extra.contains_key("background_tasks"));
    assert!(payload.extra.contains_key("session_crons"));
}

#[test]
fn payload_missing_required_field_is_an_error_not_a_panic() {
    let raw = serde_json::json!({"session_id": "s", "cwd": "/tmp/lab"});
    assert!(HookPayload::parse(&raw).is_err(), "missing hook_event_name");
    let raw = serde_json::json!({"hook_event_name": "Stop", "cwd": "/tmp/lab"});
    assert!(HookPayload::parse(&raw).is_err(), "missing session_id");
    let raw = serde_json::json!({"hook_event_name": "Stop", "session_id": "s"});
    assert!(HookPayload::parse(&raw).is_err(), "missing cwd");
}

#[test]
fn payload_with_future_unknown_fields_still_parses() {
    let raw = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "s",
        "cwd": "/tmp/lab",
        "tool_name": "Bash",
        "warp_field_stabilizer": {"engaged": true}
    });
    let payload = HookPayload::parse(&raw).expect("unknown fields are tolerated");
    assert_eq!(
        payload.extra.get("warp_field_stabilizer"),
        Some(&serde_json::json!({"engaged": true}))
    );
}
