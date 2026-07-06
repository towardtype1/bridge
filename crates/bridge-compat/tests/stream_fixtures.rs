//! Fixture-driven stream-json parsing tests.
//!
//! `stream_basic.ndjson` and `stream_resume.ndjson` are REAL captures from
//! claude CLI 2.1.201; every line must parse into the correct variant.
//! `stream_errors.synthetic.ndjson` is doc-derived (see fixtures/README.md).

use bridge_compat::{ApiErrorCategory, AssistantContent, StreamEvent, parse_line};

fn fixture_lines(name: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn parse_all(name: &str) -> Vec<StreamEvent> {
    fixture_lines(name)
        .iter()
        .enumerate()
        .map(|(i, line)| parse_line(line).unwrap_or_else(|e| panic!("{name} line {}: {e}", i + 1)))
        .collect()
}

const SESSION: &str = "a2356b0f-c205-48cc-8654-88c062868484";

#[test]
fn stream_basic_parses_every_line_into_the_documented_variants() {
    let events = parse_all("stream_basic.ndjson");
    assert_eq!(events.len(), 15, "stream_basic.ndjson must have 15 lines");

    let count = |pred: fn(&StreamEvent) -> bool| events.iter().filter(|e| pred(e)).count();
    assert_eq!(count(|e| matches!(e, StreamEvent::SystemInit(_))), 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, StreamEvent::SystemOther { subtype } if subtype == "thinking_tokens"))
            .count(),
        7
    );
    assert_eq!(count(|e| matches!(e, StreamEvent::Assistant(_))), 4);
    assert_eq!(count(|e| matches!(e, StreamEvent::UserToolResult(_))), 1);
    assert_eq!(count(|e| matches!(e, StreamEvent::RateLimit(_))), 1);
    assert_eq!(count(|e| matches!(e, StreamEvent::Result(_))), 1);
    assert_eq!(count(|e| matches!(e, StreamEvent::Unknown { .. })), 0);
    assert_eq!(count(|e| matches!(e, StreamEvent::ApiRetry(_))), 0);
}

#[test]
fn stream_basic_init_fields_are_extracted_including_camel_case_keys() {
    let events = parse_all("stream_basic.ndjson");
    let StreamEvent::SystemInit(init) = &events[0] else {
        panic!("line 1 must be SystemInit, got {:?}", events[0]);
    };
    assert_eq!(init.session_id, SESSION);
    assert_eq!(init.model, "claude-haiku-4-5-20251001");
    assert!(init.tools.iter().any(|t| t == "Bash"));
    assert!(init.tools.iter().any(|t| t == "Edit"));
    assert_eq!(init.claude_code_version.as_deref(), Some("2.1.201"));
    // camelCase in the wire format: apiKeySource / permissionMode
    assert_eq!(init.api_key_source.as_deref(), Some("none"));
    assert_eq!(init.permission_mode.as_deref(), Some("default"));
}

#[test]
fn stream_basic_assistant_content_blocks_are_typed() {
    let events = parse_all("stream_basic.ndjson");
    let assistants: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Assistant(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(assistants.len(), 4);

    // Order in the fixture: thinking, tool_use, thinking, text.
    assert_eq!(assistants[0].content, vec![AssistantContent::Thinking]);
    assert_eq!(
        assistants[1].content,
        vec![AssistantContent::ToolUse {
            id: "toolu_01E4aMtwwd8YYvxiCB2PB7WD".into(),
            name: "Bash".into(),
            input: serde_json::json!({
                "command": "echo LCARS-OK",
                "description": "Run the specified echo command"
            }),
        }]
    );
    assert_eq!(assistants[2].content, vec![AssistantContent::Thinking]);
    assert_eq!(
        assistants[3].content,
        vec![AssistantContent::Text {
            text: "DONE".into()
        }]
    );
    for a in &assistants {
        assert_eq!(a.session_id, SESSION);
        assert_eq!(a.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    }
}

#[test]
fn stream_basic_user_event_carries_tool_results() {
    let events = parse_all("stream_basic.ndjson");
    let user = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::UserToolResult(u) => Some(u),
            _ => None,
        })
        .expect("one user event");
    assert_eq!(user.session_id, SESSION);
    assert_eq!(
        user.tool_results,
        vec![("toolu_01E4aMtwwd8YYvxiCB2PB7WD".to_owned(), false)]
    );
}

#[test]
fn stream_basic_rate_limit_event_unnests_rate_limit_info() {
    let events = parse_all("stream_basic.ndjson");
    let rl = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::RateLimit(r) => Some(r),
            _ => None,
        })
        .expect("one rate_limit_event");
    assert_eq!(rl.status, "allowed");
    assert_eq!(rl.resets_at, Some(1785542400));
    assert_eq!(rl.rate_limit_type.as_deref(), Some("overage"));
}

#[test]
fn stream_basic_result_success_fields() {
    let events = parse_all("stream_basic.ndjson");
    let StreamEvent::Result(res) = events.last().unwrap() else {
        panic!("last line must be Result, got {:?}", events.last());
    };
    assert_eq!(res.subtype, "success");
    assert!(!res.is_error);
    assert_eq!(res.session_id.as_deref(), Some(SESSION));
    assert_eq!(res.num_turns, Some(2));
    assert_eq!(res.result.as_deref(), Some("DONE"));
    assert_eq!(res.total_cost_usd, Some(0.0546649));
    assert_eq!(res.duration_ms, Some(4364));
    assert_eq!(res.api_error_status, None, "null must parse as None");
    assert_eq!(res.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(res.terminal_reason.as_deref(), Some("completed"));
    assert_eq!(res.structured_output, None);
    let usage = res.usage.as_ref().expect("usage totals");
    assert_eq!(usage.input_tokens, Some(16));
    assert_eq!(usage.output_tokens, Some(183));
    assert_eq!(usage.cache_read_input_tokens, Some(24899));
    assert_eq!(usage.cache_creation_input_tokens, Some(25072));
    assert!(res.permission_denials.is_empty());
    assert!(!res.looks_rate_limited());
}

#[test]
fn stream_resume_parses_and_retains_the_session_id() {
    let events = parse_all("stream_resume.ndjson");
    assert_eq!(events.len(), 9, "stream_resume.ndjson must have 9 lines");

    let count = |pred: fn(&StreamEvent) -> bool| events.iter().filter(|e| pred(e)).count();
    assert_eq!(count(|e| matches!(e, StreamEvent::SystemInit(_))), 1);
    assert_eq!(count(|e| matches!(e, StreamEvent::SystemOther { .. })), 4);
    assert_eq!(count(|e| matches!(e, StreamEvent::Assistant(_))), 2);
    assert_eq!(count(|e| matches!(e, StreamEvent::RateLimit(_))), 1);
    assert_eq!(count(|e| matches!(e, StreamEvent::Result(_))), 1);
    assert_eq!(count(|e| matches!(e, StreamEvent::Unknown { .. })), 0);

    let StreamEvent::Result(res) = events.last().unwrap() else {
        panic!("last line must be Result");
    };
    // --resume keeps the SAME session_id (plan ground truth item 10).
    assert_eq!(res.session_id.as_deref(), Some(SESSION));
    assert_eq!(res.num_turns, Some(1), "num_turns counts only new turns");
}

#[test]
fn synthetic_api_retry_maps_error_category() {
    let events = parse_all("stream_errors.synthetic.ndjson");
    let StreamEvent::ApiRetry(retry) = &events[0] else {
        panic!("line 1 must be ApiRetry, got {:?}", events[0]);
    };
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.max_retries, Some(10));
    assert_eq!(retry.retry_delay_ms, Some(5000));
    assert_eq!(retry.error_status, Some(429));
    assert_eq!(retry.error, ApiErrorCategory::RateLimit);
}

#[test]
fn synthetic_rate_limit_event_with_non_allowed_status() {
    let events = parse_all("stream_errors.synthetic.ndjson");
    let StreamEvent::RateLimit(rl) = &events[1] else {
        panic!("line 2 must be RateLimit, got {:?}", events[1]);
    };
    assert_eq!(rl.status, "rejected");
    assert_eq!(rl.resets_at, Some(1785546000));
    assert_eq!(rl.rate_limit_type.as_deref(), Some("five_hour"));
}

#[test]
fn synthetic_error_result_parses_with_missing_optional_fields() {
    let events = parse_all("stream_errors.synthetic.ndjson");
    let StreamEvent::Result(res) = &events[2] else {
        panic!("line 3 must be Result, got {:?}", events[2]);
    };
    assert_eq!(res.subtype, "error_max_turns");
    assert!(res.is_error);
    assert_eq!(res.num_turns, Some(3));
    assert_eq!(res.result, None, "error results may omit the final text");
    assert_eq!(res.api_error_status, None);
    // Max-turns exhaustion is NOT a rate limit; the heuristic stays quiet.
    assert!(!res.looks_rate_limited());
}
