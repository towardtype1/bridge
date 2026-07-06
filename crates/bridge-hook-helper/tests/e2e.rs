//! End-to-end tests driving the real bridge-hook-helper binary.
//!
//! A stub HTTP server (plain std::net::TcpListener in a thread) plays the
//! control server. Every failure-path test asserts exit code 0: the helper
//! is fail-closed, and a non-zero exit would surface as a hook error
//! instead of the deny decision it printed.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_bridge-hook-helper");

const PRETOOLUSE: &str = include_str!("fixtures/hook_pretooluse.json");
const POSTTOOLUSE: &str = include_str!("fixtures/hook_posttooluse.json");
const STOP: &str = include_str!("fixtures/hook_stop.json");

const WS_ID: &str = "1f0d7f2a-9f0f-4a4a-8b6a-2f2a7c9d1e3b";
const TOKEN: &str = "sekret-token";

/// What the stub control server does after reading one request.
enum StubBehavior {
    Respond { status: u16, body: &'static str },
    SleepThenClose(Duration),
}

/// Spawns a one-shot stub server. Returns its base URL and a channel that
/// yields the raw request bytes it captured.
fn spawn_stub(behavior: StubBehavior) -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request(&mut stream);
            let _ = tx.send(request);
            match behavior {
                StubBehavior::Respond { status, body } => {
                    let reason = if status == 200 { "OK" } else { "Internal Server Error" };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
                StubBehavior::SleepThenClose(delay) => thread::sleep(delay),
            }
        }
    });
    (format!("http://{addr}"), rx)
}

/// Reads one HTTP/1.1 request (headers + Content-Length body) off the stream.
fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let headers_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return buf,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let headers = String::from_utf8_lossy(&buf[..headers_end]).to_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < headers_end + content_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    buf
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A URL whose port was just released, so connections are refused.
fn refused_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    format!("http://{addr}")
}

/// Runs the helper binary with a scrubbed BRIDGE_* environment plus the
/// given env/args, piping `stdin_bytes` to stdin.
fn run_helper(stdin_bytes: &[u8], envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(BIN);
    for var in [
        "BRIDGE_SERVER_URL",
        "BRIDGE_WORKSTREAM_ID",
        "BRIDGE_TOKEN",
        "BRIDGE_HELPER_TIMEOUT_MS",
    ] {
        cmd.env_remove(var);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.args(args);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn helper");
    child
        .stdin
        .take()
        .expect("stdin handle")
        .write_all(stdin_bytes)
        .expect("write stdin");
    child.wait_with_output().expect("wait for helper")
}

fn base_env(url: &str) -> Vec<(&str, &str)> {
    vec![
        ("BRIDGE_SERVER_URL", url),
        ("BRIDGE_WORKSTREAM_ID", WS_ID),
        ("BRIDGE_TOKEN", TOKEN),
    ]
}

/// Asserts stdout is a PreToolUse deny decision with a non-empty reason.
fn assert_pretooluse_deny(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "helper must exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr));
    let decision: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON ({e}): {:?}", String::from_utf8_lossy(&output.stdout)));
    let hso = &decision["hookSpecificOutput"];
    assert_eq!(hso["hookEventName"], "PreToolUse");
    assert_eq!(hso["permissionDecision"], "deny");
    let reason = hso["permissionDecisionReason"]
        .as_str()
        .expect("permissionDecisionReason must be a string");
    assert!(!reason.is_empty(), "deny must carry a reason");
}

/// Asserts stdout is exactly `{}` (no opinion; PostToolUse/Stop fallback).
fn assert_empty_object(output: &Output) {
    assert_eq!(output.status.code(), Some(0));
    let decision: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(decision, serde_json::json!({}));
}

#[test]
fn happy_path_prints_server_body_verbatim_and_posts_wire_request() {
    let body = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"tactical says no"}}"#;
    let (url, rx) = spawn_stub(StubBehavior::Respond { status: 200, body });

    let output = run_helper(PRETOOLUSE.as_bytes(), &base_env(&url), &[]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout,
        body.as_bytes(),
        "stdout must be the response body verbatim"
    );

    let captured = rx.recv_timeout(Duration::from_secs(5)).expect("request captured");
    let text = String::from_utf8_lossy(&captured).into_owned();
    let headers_end = text.find("\r\n\r\n").expect("request has headers");
    let head = text[..headers_end].to_lowercase();
    assert!(
        head.starts_with("post /hook http/1.1"),
        "must POST to /hook, got: {head}"
    );
    assert!(
        head.contains(&format!("authorization: bearer {TOKEN}")),
        "must carry the bearer token, got: {head}"
    );

    let wire: serde_json::Value =
        serde_json::from_slice(&captured[headers_end + 4..]).expect("wire body is JSON");
    assert_eq!(wire["workstream_id"], WS_ID);
    let expected_payload: serde_json::Value = serde_json::from_str(PRETOOLUSE).unwrap();
    assert_eq!(wire["payload"], expected_payload, "payload forwarded untouched");
}

#[test]
fn missing_env_vars_denies_pretooluse() {
    let output = run_helper(PRETOOLUSE.as_bytes(), &[], &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn partially_missing_env_denies_pretooluse() {
    // Token absent; the other two present.
    let (url, _rx) = spawn_stub(StubBehavior::Respond { status: 200, body: "{}" });
    let envs = [("BRIDGE_SERVER_URL", url.as_str()), ("BRIDGE_WORKSTREAM_ID", WS_ID)];
    let output = run_helper(PRETOOLUSE.as_bytes(), &envs, &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn connection_refused_denies_pretooluse() {
    let url = refused_url();
    let output = run_helper(PRETOOLUSE.as_bytes(), &base_env(&url), &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn server_500_denies_pretooluse() {
    let (url, _rx) = spawn_stub(StubBehavior::Respond { status: 500, body: "boom" });
    let output = run_helper(PRETOOLUSE.as_bytes(), &base_env(&url), &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn empty_200_body_denies_pretooluse() {
    let (url, _rx) = spawn_stub(StubBehavior::Respond { status: 200, body: "" });
    let output = run_helper(PRETOOLUSE.as_bytes(), &base_env(&url), &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn response_timeout_denies_pretooluse() {
    let (url, _rx) = spawn_stub(StubBehavior::SleepThenClose(Duration::from_secs(3)));
    let mut envs = base_env(&url);
    envs.push(("BRIDGE_HELPER_TIMEOUT_MS", "500"));

    let started = Instant::now();
    let output = run_helper(PRETOOLUSE.as_bytes(), &envs, &[]);
    let elapsed = started.elapsed();

    assert_pretooluse_deny(&output);
    assert!(
        elapsed < Duration::from_millis(2500),
        "timeout must fire at ~500ms, not wait for the server; took {elapsed:?}"
    );
}

#[test]
fn posttooluse_with_refused_server_prints_empty_object() {
    let url = refused_url();
    let output = run_helper(POSTTOOLUSE.as_bytes(), &base_env(&url), &[]);
    assert_empty_object(&output);
}

#[test]
fn stop_with_refused_server_prints_empty_object() {
    let url = refused_url();
    let output = run_helper(STOP.as_bytes(), &base_env(&url), &[]);
    assert_empty_object(&output);
}

#[test]
fn posttooluse_with_missing_env_prints_empty_object() {
    let output = run_helper(POSTTOOLUSE.as_bytes(), &[], &[]);
    assert_empty_object(&output);
}

#[test]
fn garbage_stdin_denies_pretooluse_style() {
    // Binary garbage: invalid UTF-8, certainly not JSON. No server needed;
    // the helper must not even try to forward it.
    let garbage: &[u8] = &[0xff, 0xfe, 0x00, 0x9b, 0x13, 0x37, 0xde, 0xad];
    let url = refused_url();
    let output = run_helper(garbage, &base_env(&url), &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn empty_stdin_denies_pretooluse_style() {
    let url = refused_url();
    let output = run_helper(b"", &base_env(&url), &[]);
    assert_pretooluse_deny(&output);
}

#[test]
fn argv_overrides_beat_env() {
    let body = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"escalation approved"}}"#;
    let (good_url, rx) = spawn_stub(StubBehavior::Respond { status: 200, body });
    let bad_url = refused_url();

    let argv_ws = "9e107d9d-372b-4c81-a1f0-9a2b3c4d5e6f";
    let argv_token = "argv-token";
    let envs = [
        ("BRIDGE_SERVER_URL", bad_url.as_str()),
        ("BRIDGE_WORKSTREAM_ID", WS_ID),
        ("BRIDGE_TOKEN", "env-token"),
    ];
    let args = [
        "--server-url",
        good_url.as_str(),
        "--workstream",
        argv_ws,
        "--token",
        argv_token,
    ];
    let output = run_helper(PRETOOLUSE.as_bytes(), &envs, &args);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, body.as_bytes());

    let captured = rx.recv_timeout(Duration::from_secs(5)).expect("request captured");
    let text = String::from_utf8_lossy(&captured).into_owned();
    let headers_end = text.find("\r\n\r\n").expect("request has headers");
    let head = text[..headers_end].to_lowercase();
    assert!(
        head.contains(&format!("authorization: bearer {argv_token}")),
        "argv token must beat env token, got: {head}"
    );
    let wire: serde_json::Value =
        serde_json::from_slice(&captured[headers_end + 4..]).expect("wire body is JSON");
    assert_eq!(wire["workstream_id"], argv_ws, "argv workstream must beat env");
}
