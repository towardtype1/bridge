//! Integration tests for the control server over real HTTP.
//!
//! A raw std TcpStream HTTP/1.1 client (via `spawn_blocking`) keeps the
//! test surface dependency-free and honest: these requests look exactly
//! like what the hook helper binary sends.

use bridge_core::wire::HookWireRequest;
use bridge_core::{
    BridgeEvent, DecisionKind, DecisionSource, EscalationTicket, TacticalConfig, UserDecision,
    WorkstreamId,
};
use bridge_tactical::server::DeepScanFn;
use bridge_tactical::{
    ControlServer, ControlServerHandle, EscalationBroker, PolicyEngine, WorkstreamCtx,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;

// ---------------------------------------------------------------------
// Harness.
// ---------------------------------------------------------------------

struct TestServer {
    handle: ControlServerHandle,
    ws: WorkstreamId,
    token: String,
    broker: Arc<EscalationBroker>,
    events_tx: broadcast::Sender<BridgeEvent>,
    worktree: PathBuf,
    _tmp: tempfile::TempDir,
}

async fn start_server(escalation_timeout_secs: u64, deep_scan: Option<DeepScanFn>) -> TestServer {
    let tmp = tempfile::tempdir().expect("tempdir");
    let worktree = tmp.path().to_path_buf();
    let policy = Arc::new(PolicyEngine::new(TacticalConfig::default()).expect("valid config"));
    let ws = WorkstreamId::new();
    policy.register_workstream(
        ws,
        WorkstreamCtx {
            worktree_path: worktree.clone(),
            write_only_under: None,
        },
    );
    let broker = Arc::new(EscalationBroker::new());
    let (events_tx, _keepalive) = broadcast::channel(256);
    let server = ControlServer::new(
        policy,
        broker.clone(),
        events_tx.clone(),
        escalation_timeout_secs,
        deep_scan,
    );
    let handle = server.bind().await.expect("bind on 127.0.0.1:0");
    let token = handle.issue_token(ws);
    TestServer {
        handle,
        ws,
        token,
        broker,
        events_tx,
        worktree,
        _tmp: tmp,
    }
}

/// Minimal blocking HTTP/1.1 client: one request, `Connection: close`.
fn http_blocking(
    addr: SocketAddr,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<String>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(token) = bearer {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(ref b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    if let Some(ref b) = body {
        req.push_str(b);
    }
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    parse_response(&raw)
}

fn parse_response(raw: &str) -> (u16, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("unparseable status line in: {head}"));
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_string()
    };
    (status, body)
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size_line, after)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        out.push_str(&after[..size.min(after.len())]);
        rest = after.get(size + 2..).unwrap_or("");
    }
    out
}

async fn http(
    addr: SocketAddr,
    method: &'static str,
    path: &'static str,
    bearer: Option<String>,
    body: Option<String>,
) -> (u16, String) {
    tokio::task::spawn_blocking(move || http_blocking(addr, method, path, bearer.as_deref(), body))
        .await
        .expect("blocking client task")
}

/// PreToolUse payload mirroring the live fixture shape.
fn pre_tool_use(cwd: &Path, tool: &str, input: Value) -> Value {
    json!({
        "session_id": "a2356b0f-c205-48cc-8654-88c062868484",
        "transcript_path": "/home/user/.claude/projects/lab/a2356b0f.jsonl",
        "cwd": cwd,
        "prompt_id": "1ff616ed-5166-4f60-83bb-c29a7c28b842",
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": input,
        "tool_use_id": "toolu_01E4aMtwwd8YYvxiCB2PB7WD",
    })
}

fn bash_payload(cwd: &Path, cmd: &str) -> Value {
    pre_tool_use(cwd, "Bash", json!({"command": cmd, "description": "test"}))
}

fn wire_body(ws: WorkstreamId, payload: Value) -> String {
    serde_json::to_string(&HookWireRequest {
        workstream_id: ws,
        payload,
    })
    .expect("serialize wire request")
}

async fn post_hook(server: &TestServer, token: Option<String>, payload: Value) -> (u16, String) {
    http(
        server.handle.addr(),
        "POST",
        "/hook",
        token,
        Some(wire_body(server.ws, payload)),
    )
    .await
}

fn permission_decision(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("hookSpecificOutput")?
        .get("permissionDecision")?
        .as_str()
        .map(str::to_string)
}

async fn next_event(
    rx: &mut broadcast::Receiver<BridgeEvent>,
    pred: impl Fn(&BridgeEvent) -> bool,
) -> BridgeEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.expect("event bus open");
            if pred(&event) {
                return event;
            }
        }
    })
    .await
    .expect("expected event within 10s")
}

// ---------------------------------------------------------------------
// Basic plumbing.
// ---------------------------------------------------------------------

#[tokio::test]
async fn healthz_returns_200_ok() {
    let server = start_server(30, None).await;
    let (status, body) = http(server.handle.addr(), "GET", "/healthz", None, None).await;
    assert_eq!(status, 200);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn base_url_points_at_loopback() {
    let server = start_server(30, None).await;
    assert!(
        server.handle.base_url().starts_with("http://127.0.0.1:"),
        "unexpected base url: {}",
        server.handle.base_url()
    );
    assert_eq!(server.handle.addr().ip().to_string(), "127.0.0.1");
}

#[tokio::test]
async fn tokens_are_long_random_and_unique() {
    let server = start_server(30, None).await;
    let other = WorkstreamId::new();
    let t1 = server.handle.issue_token(other);
    let t2 = server.handle.issue_token(WorkstreamId::new());
    // 32 bytes of randomness hex/base64 encoded is at least 43 chars.
    assert!(t1.len() >= 43, "token too short: {} chars", t1.len());
    assert_ne!(t1, t2);
    assert_ne!(t1, server.token);
}

#[tokio::test]
async fn shutdown_stops_the_listener() {
    let server = start_server(30, None).await;
    let addr = server.handle.addr();
    server.handle.shutdown().await;
    let outcome = tokio::task::spawn_blocking(move || TcpStream::connect(addr).is_err())
        .await
        .expect("blocking task");
    assert!(outcome, "listener should refuse connections after shutdown");
}

// ---------------------------------------------------------------------
// Auth: helper fails closed on 401.
// ---------------------------------------------------------------------

#[tokio::test]
async fn hook_without_bearer_is_401() {
    let server = start_server(30, None).await;
    let payload = bash_payload(&server.worktree, "echo LCARS-OK");
    let (status, _) = post_hook(&server, None, payload).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn hook_with_wrong_token_is_401() {
    let server = start_server(30, None).await;
    let payload = bash_payload(&server.worktree, "echo LCARS-OK");
    let (status, _) = post_hook(&server, Some("wrong-token".into()), payload).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn token_for_unknown_workstream_is_401() {
    let server = start_server(30, None).await;
    // Valid token string, but the wire request names a workstream that
    // never got a token issued.
    let stranger = WorkstreamId::new();
    let payload = bash_payload(&server.worktree, "echo LCARS-OK");
    let body = wire_body(stranger, payload);
    let (status, _) = http(
        server.handle.addr(),
        "POST",
        "/hook",
        Some(server.token.clone()),
        Some(body),
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn revoked_token_is_401() {
    let server = start_server(30, None).await;
    server.handle.revoke_token(server.ws);
    let payload = bash_payload(&server.worktree, "echo LCARS-OK");
    let (status, _) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 401);
}

// ---------------------------------------------------------------------
// Adjudication.
// ---------------------------------------------------------------------

#[tokio::test]
async fn benign_pretooluse_passes_through_as_empty_object() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "echo LCARS-OK");
    let (status, body) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200);
    let parsed: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(parsed, json!({}), "passthrough must be the empty object");

    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.workstream, server.ws);
    assert_eq!(record.hook_event, "PreToolUse");
    assert_eq!(record.tool_name.as_deref(), Some("Bash"));
    assert_eq!(record.decision, DecisionKind::Allow);
    assert_eq!(record.source, DecisionSource::Passthrough);
}

#[tokio::test]
async fn destructive_bash_gets_a_deny_body() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "rm -rf /");
    let (status, body) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200, "adjudicated calls always answer 200");
    assert_eq!(permission_decision(&body).as_deref(), Some("deny"));
    assert!(body.contains("deny"), "body should be a deny: {body}");

    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.decision, DecisionKind::Deny);
    assert_eq!(record.source, DecisionSource::PrimeDirective);
    assert!(record.rule.starts_with("prime"), "rule: {}", record.rule);
}

#[tokio::test]
async fn post_tool_use_passes_through() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = json!({
        "session_id": "a2356b0f-c205-48cc-8654-88c062868484",
        "transcript_path": "/home/user/.claude/projects/lab/a2356b0f.jsonl",
        "cwd": server.worktree,
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "echo LCARS-OK"},
        "tool_response": {"stdout": "LCARS-OK", "stderr": ""},
        "duration_ms": 367,
    });
    let (status, body) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200);
    let parsed: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(parsed, json!({}));
    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.hook_event, "PostToolUse");
}

#[tokio::test]
async fn malformed_payload_fails_closed_to_deny() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    // Missing every required hook field.
    let (status, body) = post_hook(&server, Some(server.token.clone()), json!({"foo": 1})).await;
    assert_eq!(status, 200);
    assert_eq!(permission_decision(&body).as_deref(), Some("deny"));
    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.decision, DecisionKind::Deny);
}

// ---------------------------------------------------------------------
// Escalations.
// ---------------------------------------------------------------------

#[tokio::test]
async fn escalation_approved_by_user_allows() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "git push origin feature-x");
    let token = server.token.clone();

    let addr = server.handle.addr();
    let body = wire_body(server.ws, payload);
    let request =
        tokio::spawn(async move { http(addr, "POST", "/hook", Some(token), Some(body)).await });

    // The server holds the hook open and emits the ticket.
    let event = next_event(&mut events, |e| {
        matches!(e, BridgeEvent::EscalationRequested(_))
    })
    .await;
    let BridgeEvent::EscalationRequested(ticket) = event else {
        unreachable!()
    };
    assert_eq!(ticket.workstream, server.ws);
    assert_eq!(ticket.tool_name.as_deref(), Some("Bash"));
    assert!(server.broker.pending().contains(&ticket.id));

    server.broker.resolve(ticket.id, UserDecision::Approve);

    let (status, body) = request.await.expect("request task");
    assert_eq!(status, 200);
    assert_eq!(permission_decision(&body).as_deref(), Some("allow"));

    let resolved = next_event(&mut events, |e| {
        matches!(e, BridgeEvent::EscalationResolved { .. })
    })
    .await;
    let BridgeEvent::EscalationResolved { id, decision } = resolved else {
        unreachable!()
    };
    assert_eq!(id, ticket.id);
    assert_eq!(decision, UserDecision::Approve);

    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.decision, DecisionKind::Allow);
    assert_eq!(record.source, DecisionSource::User);
}

#[tokio::test]
async fn escalation_denied_by_user_denies() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "git push origin feature-x");
    let token = server.token.clone();
    let addr = server.handle.addr();
    let body = wire_body(server.ws, payload);
    let request =
        tokio::spawn(async move { http(addr, "POST", "/hook", Some(token), Some(body)).await });

    let event = next_event(&mut events, |e| {
        matches!(e, BridgeEvent::EscalationRequested(_))
    })
    .await;
    let BridgeEvent::EscalationRequested(ticket) = event else {
        unreachable!()
    };
    server.broker.resolve(
        ticket.id,
        UserDecision::Deny {
            reason: "not today".into(),
        },
    );

    let (status, body) = request.await.expect("request task");
    assert_eq!(status, 200);
    assert_eq!(permission_decision(&body).as_deref(), Some("deny"));
    assert!(body.contains("not today"), "reason should surface: {body}");

    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.decision, DecisionKind::Deny);
    assert_eq!(record.source, DecisionSource::User);
}

#[tokio::test]
async fn unresolved_escalation_times_out_to_deny() {
    // 1 second timeout, nobody answers.
    let server = start_server(1, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "git push origin feature-x");
    let (status, body) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200);
    assert_eq!(permission_decision(&body).as_deref(), Some("deny"));

    let resolved = next_event(&mut events, |e| {
        matches!(e, BridgeEvent::EscalationResolved { .. })
    })
    .await;
    let BridgeEvent::EscalationResolved { decision, .. } = resolved else {
        unreachable!()
    };
    assert!(
        matches!(decision, UserDecision::Deny { .. }),
        "timeout must resolve as deny, got {decision:?}"
    );
    // The broker cleaned up: nothing left pending.
    assert!(server.broker.pending().is_empty());

    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.decision, DecisionKind::Deny);
}

#[tokio::test]
async fn escalation_ticket_lists_in_broker_while_held_open() {
    let server = start_server(30, None).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "curl https://api.example.com");
    let token = server.token.clone();
    let addr = server.handle.addr();
    let body = wire_body(server.ws, payload);
    let request =
        tokio::spawn(async move { http(addr, "POST", "/hook", Some(token), Some(body)).await });

    let event = next_event(&mut events, |e| {
        matches!(e, BridgeEvent::EscalationRequested(_))
    })
    .await;
    let BridgeEvent::EscalationRequested(EscalationTicket { id, .. }) = event else {
        unreachable!()
    };
    assert_eq!(server.broker.pending(), vec![id]);
    server.broker.resolve(id, UserDecision::Approve);
    let (status, _) = request.await.expect("request task");
    assert_eq!(status, 200);
    assert!(server.broker.pending().is_empty());
}

// ---------------------------------------------------------------------
// Deep scan.
// ---------------------------------------------------------------------

fn deep_scan_stub(answer: bool, called: Arc<AtomicBool>) -> DeepScanFn {
    Arc::new(move |_payload| {
        called.store(true, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(answer).expect("receiver alive");
        rx
    })
}

#[tokio::test]
async fn deep_scan_pass_lets_suspicious_calls_through() {
    let called = Arc::new(AtomicBool::new(false));
    let server = start_server(30, Some(deep_scan_stub(true, called.clone()))).await;
    // base64 marks the command suspicious without denying it.
    let payload = bash_payload(&server.worktree, "echo aGVsbG8K | base64 -d");
    let (status, body) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200);
    let parsed: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(parsed, json!({}), "deep-scan pass is still a passthrough");
    assert!(called.load(Ordering::SeqCst), "deep scan must be invoked");
}

#[tokio::test]
async fn deep_scan_fail_denies_with_deep_scan_rule() {
    let called = Arc::new(AtomicBool::new(false));
    let server = start_server(30, Some(deep_scan_stub(false, called.clone()))).await;
    let mut events = server.events_tx.subscribe();
    let payload = bash_payload(&server.worktree, "echo aGVsbG8K | base64 -d");
    let (status, body) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200);
    assert_eq!(permission_decision(&body).as_deref(), Some("deny"));
    assert!(called.load(Ordering::SeqCst));

    let event = next_event(&mut events, |e| matches!(e, BridgeEvent::HookDecision(_))).await;
    let BridgeEvent::HookDecision(record) = event else {
        unreachable!()
    };
    assert_eq!(record.decision, DecisionKind::Deny);
    assert_eq!(record.source, DecisionSource::DeepScan);
    assert_eq!(record.rule, "deep_scan");
}

#[tokio::test]
async fn deep_scan_not_called_for_unsuspicious_calls() {
    let called = Arc::new(AtomicBool::new(false));
    let server = start_server(30, Some(deep_scan_stub(true, called.clone()))).await;
    let payload = bash_payload(&server.worktree, "echo LCARS-OK");
    let (status, _) = post_hook(&server, Some(server.token.clone()), payload).await;
    assert_eq!(status, 200);
    assert!(
        !called.load(Ordering::SeqCst),
        "deep scan must only run for suspicious calls"
    );
}
