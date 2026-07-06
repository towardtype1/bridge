//! bridge-hook-helper: the fail-closed hook forwarder.
//!
//! Claude Code runs this binary for PreToolUse/PostToolUse/Stop hooks in
//! Bridge-managed worktrees. It:
//! 1. reads the hook JSON payload from stdin (hard cap 10 MiB),
//! 2. reads `BRIDGE_SERVER_URL`, `BRIDGE_WORKSTREAM_ID`, `BRIDGE_TOKEN`
//!    from env (argv overrides: `--server-url`, `--workstream`, `--token`),
//! 3. POSTs `{workstream_id, payload}` to `<url>/hook` with
//!    `Authorization: Bearer <token>`, timeout `BRIDGE_HELPER_TIMEOUT_MS`
//!    (default 570_000 ms; must stay under the installed hook timeout),
//! 4. prints the response body verbatim to stdout and exits 0.
//!
//! FAIL CLOSED: on ANY failure (missing env, unreadable stdin, connect
//! error, non-2xx, timeout, empty body) it prints a deny decision itself
//! and still exits 0 so the deny JSON is honored:
//! - PreToolUse payloads: permissionDecision "deny" with the failure
//!   reason (the run continues, the tool call does not).
//! - PostToolUse/Stop payloads (nothing to gate): `{}` so observability
//!   loss never corrupts the run.
//!
//! The payload's own `hook_event_name` decides which shape to print; if
//! even that is unreadable, print the PreToolUse deny (safest). Unknown
//! event names also get the deny: only PostToolUse/Stop are known-safe
//! to pass through.
//!
//! Keep this binary tiny and fast: no tokio, no async, blocking ureq only.

use std::io::Read;
use std::time::Duration;

use bridge_core::WorkstreamId;
use bridge_core::wire::{
    ENV_HELPER_TIMEOUT_MS, ENV_SERVER_URL, ENV_TOKEN, ENV_WORKSTREAM_ID, HookWireRequest,
};

/// Default HTTP timeout. The installed hook timeout is 600s; staying at
/// 570s guarantees the helper prints its own deny before Claude Code gives
/// up on the hook.
const DEFAULT_TIMEOUT_MS: u64 = 570_000;

/// Hard cap on the stdin payload size.
const STDIN_CAP_BYTES: u64 = 10 * 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Belt and braces: even an unexpected panic must not turn into a
    // non-zero exit, because that would surface as a hook error instead
    // of a decision. catch_unwind keeps the fail-closed promise.
    let output = std::panic::catch_unwind(|| run(&args))
        .unwrap_or_else(|_| deny_output("bridge-hook-helper: internal panic"));

    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&output);
    let _ = stdout.flush();
    std::process::exit(0);
}

/// Full helper flow; returns exactly the bytes to print on stdout.
/// Never panics on malformed input: every failure becomes a fallback
/// decision shaped by the payload's `hook_event_name`.
fn run(args: &[String]) -> Vec<u8> {
    let overrides = parse_args(args);

    let payload = match read_stdin_payload() {
        Ok(payload) => payload,
        // Unreadable stdin: we cannot even tell which hook fired, so the
        // PreToolUse deny is the only safe shape.
        Err(reason) => return fallback_output(None, &reason),
    };
    let event = payload
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let config = match resolve_config(&overrides, |key| {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }) {
        Ok(config) => config,
        Err(reason) => return fallback_output(event.as_deref(), &reason),
    };

    match forward(&config, &payload) {
        Ok(body) => body,
        Err(reason) => fallback_output(event.as_deref(), &reason),
    }
}

/// Argv overrides for the three connection settings. Unknown arguments are
/// ignored (schema tolerance: a future settings renderer may add flags).
#[derive(Debug, Default, PartialEq)]
struct Overrides {
    server_url: Option<String>,
    workstream: Option<String>,
    token: Option<String>,
}

/// Parses `--server-url`, `--workstream`, `--token` in both `--flag value`
/// and `--flag=value` forms. Later occurrences win; a trailing flag with no
/// value is ignored.
fn parse_args(args: &[String]) -> Overrides {
    let mut overrides = Overrides::default();
    let mut i = 0;
    while i < args.len() {
        let (name, inline_value) = match args[i].split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (args[i].as_str(), None),
        };
        let slot = match name {
            "--server-url" => Some(&mut overrides.server_url),
            "--workstream" => Some(&mut overrides.workstream),
            "--token" => Some(&mut overrides.token),
            _ => None,
        };
        if let Some(slot) = slot {
            match inline_value {
                Some(value) => *slot = Some(value),
                None => {
                    if i + 1 < args.len() {
                        *slot = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
            }
        }
        i += 1;
    }
    overrides
}

/// Fully resolved connection settings.
#[derive(Debug)]
struct HelperConfig {
    hook_url: String,
    workstream_id: WorkstreamId,
    token: String,
    timeout: Duration,
}

/// Resolves settings from argv overrides first, env second. Empty values
/// count as missing. `env` is injected so unit tests never touch the real
/// process environment.
fn resolve_config(
    overrides: &Overrides,
    env: impl Fn(&str) -> Option<String>,
) -> Result<HelperConfig, String> {
    let server_url = overrides
        .server_url
        .clone()
        .or_else(|| env(ENV_SERVER_URL))
        .ok_or_else(|| format!("{ENV_SERVER_URL} not set and no --server-url given"))?;
    let workstream_raw = overrides
        .workstream
        .clone()
        .or_else(|| env(ENV_WORKSTREAM_ID))
        .ok_or_else(|| format!("{ENV_WORKSTREAM_ID} not set and no --workstream given"))?;
    let workstream_id: WorkstreamId = workstream_raw
        .parse()
        .map_err(|e| format!("workstream id {workstream_raw:?} is not a UUID: {e}"))?;
    let token = overrides
        .token
        .clone()
        .or_else(|| env(ENV_TOKEN))
        .ok_or_else(|| format!("{ENV_TOKEN} not set and no --token given"))?;
    let timeout = Duration::from_millis(resolve_timeout_ms(env(ENV_HELPER_TIMEOUT_MS)));
    Ok(HelperConfig {
        hook_url: hook_url(&server_url),
        workstream_id,
        token,
        timeout,
    })
}

/// Timeout from `BRIDGE_HELPER_TIMEOUT_MS`; unset or unparsable falls back
/// to the default (a bad override must not weaken the deadline guarantee).
fn resolve_timeout_ms(env_value: Option<String>) -> u64 {
    env_value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS)
}

/// Joins the base server URL with the `/hook` route, tolerating trailing
/// slashes on the configured base.
fn hook_url(base: &str) -> String {
    format!("{}/hook", base.trim_end_matches('/'))
}

/// Reads stdin (capped) and parses it as JSON. Any read or parse failure is
/// an error string, never a panic.
fn read_stdin_payload() -> Result<serde_json::Value, String> {
    let mut raw = Vec::new();
    std::io::stdin()
        .lock()
        .take(STDIN_CAP_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("failed reading stdin: {e}"))?;
    if raw.len() as u64 > STDIN_CAP_BYTES {
        return Err("stdin payload exceeds the 10 MiB cap".to_owned());
    }
    parse_payload(&raw)
}

/// Parses the raw stdin bytes as a JSON value. The payload is forwarded
/// opaquely, so any valid JSON is accepted; field access stays tolerant.
fn parse_payload(raw: &[u8]) -> Result<serde_json::Value, String> {
    serde_json::from_slice(raw).map_err(|e| format!("stdin was not valid hook JSON: {e}"))
}

/// POSTs the wire request and returns the response body verbatim.
/// Non-2xx statuses, timeouts, connection failures and empty bodies are all
/// errors (ureq maps non-2xx to `Error::StatusCode` by default).
fn forward(config: &HelperConfig, payload: &serde_json::Value) -> Result<Vec<u8>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.timeout))
        .build()
        .new_agent();
    let request = HookWireRequest {
        workstream_id: config.workstream_id,
        payload: payload.clone(),
    };
    let mut response = agent
        .post(&config.hook_url)
        .header("Authorization", format!("Bearer {}", config.token))
        .send_json(&request)
        .map_err(|e| format!("control server request failed: {e}"))?;
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|e| format!("failed reading control server response: {e}"))?;
    if body.is_empty() {
        return Err("control server returned an empty body".to_owned());
    }
    Ok(body)
}

/// The local decision printed when forwarding failed. PostToolUse and Stop
/// have nothing to gate, so they pass through with `{}`; everything else
/// (PreToolUse, unknown events, unreadable payloads) gets the deny.
fn fallback_output(event: Option<&str>, reason: &str) -> Vec<u8> {
    match event {
        Some("PostToolUse") | Some("Stop") => b"{}".to_vec(),
        _ => deny_output(&format!("bridge-hook-helper: {reason}")),
    }
}

/// Builds the PreToolUse deny decision.
///
/// The shape is pinned to the Claude Code 2.1.x hook response schema and
/// deliberately duplicated from bridge-compat: the helper must keep working
/// standalone even when compat evolves, so it constructs its own deny JSON.
fn deny_output(reason: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }))
    .unwrap_or_else(|_| {
        // Unreachable for this literal shape; kept so the binary can never
        // panic out of the fail-closed path.
        br#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"bridge-hook-helper: deny serialization failed"}}"#.to_vec()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn no_env(_key: &str) -> Option<String> {
        None
    }

    #[test]
    fn parse_args_space_separated_form() {
        let overrides = parse_args(&strings(&[
            "--server-url",
            "http://127.0.0.1:1",
            "--workstream",
            "abc",
            "--token",
            "t",
        ]));
        assert_eq!(overrides.server_url.as_deref(), Some("http://127.0.0.1:1"));
        assert_eq!(overrides.workstream.as_deref(), Some("abc"));
        assert_eq!(overrides.token.as_deref(), Some("t"));
    }

    #[test]
    fn parse_args_equals_form_and_later_wins() {
        let overrides = parse_args(&strings(&[
            "--token=first",
            "--token=second",
            "--server-url=http://x/",
        ]));
        assert_eq!(overrides.token.as_deref(), Some("second"));
        assert_eq!(overrides.server_url.as_deref(), Some("http://x/"));
        assert_eq!(overrides.workstream, None);
    }

    #[test]
    fn parse_args_ignores_unknown_flags_and_trailing_flag_without_value() {
        let overrides = parse_args(&strings(&["--future-flag", "v", "--token"]));
        assert_eq!(overrides, Overrides::default());
    }

    #[test]
    fn resolve_config_from_env_only() {
        let ws = WorkstreamId::new();
        let ws_string = ws.to_string();
        let config = resolve_config(&Overrides::default(), |key| match key {
            ENV_SERVER_URL => Some("http://127.0.0.1:9/".to_owned()),
            ENV_WORKSTREAM_ID => Some(ws_string.clone()),
            ENV_TOKEN => Some("tok".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.hook_url, "http://127.0.0.1:9/hook");
        assert_eq!(config.workstream_id, ws);
        assert_eq!(config.token, "tok");
        assert_eq!(config.timeout, Duration::from_millis(DEFAULT_TIMEOUT_MS));
    }

    #[test]
    fn resolve_config_overrides_beat_env() {
        let env_ws = WorkstreamId::new();
        let argv_ws = WorkstreamId::new();
        let overrides = Overrides {
            server_url: Some("http://argv:1".to_owned()),
            workstream: Some(argv_ws.to_string()),
            token: Some("argv-tok".to_owned()),
        };
        let env_ws_string = env_ws.to_string();
        let config = resolve_config(&overrides, |key| match key {
            ENV_SERVER_URL => Some("http://env:2".to_owned()),
            ENV_WORKSTREAM_ID => Some(env_ws_string.clone()),
            ENV_TOKEN => Some("env-tok".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.hook_url, "http://argv:1/hook");
        assert_eq!(config.workstream_id, argv_ws);
        assert_eq!(config.token, "argv-tok");
    }

    #[test]
    fn resolve_config_missing_each_var_errors_with_its_name() {
        let ws = WorkstreamId::new().to_string();
        type EnvFn<'a> = &'a dyn Fn(&str) -> Option<String>;
        let cases: [(&str, EnvFn); 3] = [
            (ENV_SERVER_URL, &|key: &str| {
                (key != ENV_SERVER_URL).then(|| ws.clone())
            }),
            (ENV_WORKSTREAM_ID, &|key: &str| {
                (key != ENV_WORKSTREAM_ID).then(|| "http://x".to_owned())
            }),
            (ENV_TOKEN, &|key: &str| match key {
                ENV_SERVER_URL => Some("http://x".to_owned()),
                ENV_WORKSTREAM_ID => Some(ws.clone()),
                _ => None,
            }),
        ];
        for (missing, env) in cases {
            let err = resolve_config(&Overrides::default(), env).unwrap_err();
            assert!(err.contains(missing), "error {err:?} must name {missing}");
        }
    }

    #[test]
    fn resolve_config_rejects_non_uuid_workstream() {
        let err = resolve_config(&Overrides::default(), |key| match key {
            ENV_SERVER_URL => Some("http://x".to_owned()),
            ENV_WORKSTREAM_ID => Some("not-a-uuid".to_owned()),
            ENV_TOKEN => Some("tok".to_owned()),
            _ => None,
        })
        .unwrap_err();
        assert!(err.contains("not-a-uuid"));
    }

    #[test]
    fn resolve_timeout_default_invalid_and_explicit() {
        assert_eq!(resolve_timeout_ms(None), DEFAULT_TIMEOUT_MS);
        assert_eq!(resolve_timeout_ms(Some("nope".to_owned())), DEFAULT_TIMEOUT_MS);
        assert_eq!(resolve_timeout_ms(Some("-5".to_owned())), DEFAULT_TIMEOUT_MS);
        assert_eq!(resolve_timeout_ms(Some("500".to_owned())), 500);
        assert_eq!(resolve_timeout_ms(Some(" 500 ".to_owned())), 500);
    }

    #[test]
    fn hook_url_joins_with_and_without_trailing_slash() {
        assert_eq!(hook_url("http://127.0.0.1:49172"), "http://127.0.0.1:49172/hook");
        assert_eq!(hook_url("http://127.0.0.1:49172/"), "http://127.0.0.1:49172/hook");
    }

    #[test]
    fn parse_payload_accepts_json_and_rejects_garbage() {
        let payload = parse_payload(br#"{"hook_event_name":"Stop","new_field":1}"#).unwrap();
        assert_eq!(payload["hook_event_name"], "Stop");
        assert!(parse_payload(&[0xff, 0x00, 0x13]).is_err());
        assert!(parse_payload(b"").is_err());
        assert!(parse_payload(b"{truncated").is_err());
    }

    #[test]
    fn fallback_is_deny_for_pretooluse_unknown_and_unreadable() {
        for event in [Some("PreToolUse"), Some("SomeFutureGate"), None] {
            let out = fallback_output(event, "boom");
            let decision: serde_json::Value = serde_json::from_slice(&out).unwrap();
            let hso = &decision["hookSpecificOutput"];
            assert_eq!(hso["hookEventName"], "PreToolUse", "event {event:?}");
            assert_eq!(hso["permissionDecision"], "deny", "event {event:?}");
            let reason = hso["permissionDecisionReason"].as_str().unwrap();
            assert!(reason.starts_with("bridge-hook-helper: "), "reason {reason:?}");
            assert!(reason.contains("boom"));
        }
    }

    #[test]
    fn fallback_is_empty_object_for_posttooluse_and_stop() {
        for event in ["PostToolUse", "Stop"] {
            assert_eq!(fallback_output(Some(event), "boom"), b"{}");
        }
    }

    #[test]
    fn forward_fails_closed_on_unresolvable_host() {
        let config = HelperConfig {
            hook_url: "http://bridge-helper-test.invalid/hook".to_owned(),
            workstream_id: WorkstreamId::new(),
            token: "tok".to_owned(),
            timeout: Duration::from_millis(500),
        };
        let err = forward(&config, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("control server request failed"), "{err}");
    }

    #[test]
    fn missing_env_reason_flows_through_run_shape() {
        // resolve_config error -> fallback -> deny with prefixed reason.
        let err = resolve_config(&Overrides::default(), no_env).unwrap_err();
        let out = fallback_output(Some("PreToolUse"), &err);
        let decision: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let reason = decision["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        assert!(reason.contains(ENV_SERVER_URL));
    }
}
