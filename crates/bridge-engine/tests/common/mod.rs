//! Shared test helpers: a fake `claude` CLI and runner plumbing.
//!
//! The fake is a POSIX shell script written into a tempdir at test time
//! (never the real CLI; it must not burn subscription quota). It reads
//! `FAKE_BEHAVIOR` (success|nonzero|hang|rate_limited), `FAKE_DELAY_MS`,
//! `FAKE_OUT` (path receiving pid + argv + env snapshot) and
//! `FAKE_OVERLAP_LOG` (shared concurrency journal), each with per-test
//! baked defaults substituted at generation time so parallel tests never
//! race on process-global env vars.

#![allow(dead_code)]

use bridge_compat::ClaudeInvocation;
use bridge_core::{BridgeEvent, ClaudeConfig, Station, WorkstreamId};
use bridge_engine::{ClaudeRunner, TurnCtx};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::sync::broadcast;

/// NDJSON shapes copied from `bridge-compat/tests/fixtures/stream_basic.ndjson`
/// (claude CLI 2.1.201), trimmed to the fields the engine consumes.
const SCRIPT_TEMPLATE: &str = r#"#!/bin/sh
# Fake claude CLI for bridge-engine tests. NEVER the real binary.
: "${FAKE_BEHAVIOR:=__BEHAVIOR__}"
: "${FAKE_DELAY_MS:=__DELAY_MS__}"
: "${FAKE_OUT:=__OUT__}"
: "${FAKE_OVERLAP_LOG:=__OVERLAP__}"

if [ "$1" = "--version" ]; then
  echo "2.1.201 (Claude Code)"
  exit 0
fi

TAG=general
case "$*" in
  *KOBAYASHI*) TAG=kobayashi ;;
esac

if [ -n "$FAKE_OUT" ]; then
  {
    echo "PID=$$"
    for arg in "$@"; do
      printf 'ARG=%s\n' "$arg"
    done
    echo "ENV_BEGIN"
    env
    echo "ENV_END"
  } > "$FAKE_OUT"
fi

if [ -n "$FAKE_OVERLAP_LOG" ]; then
  echo "S $TAG" >> "$FAKE_OVERLAP_LOG"
fi

finish() {
  if [ -n "$FAKE_OVERLAP_LOG" ]; then
    echo "E $TAG" >> "$FAKE_OVERLAP_LOG"
  fi
}

if [ "$FAKE_DELAY_MS" -gt 0 ] 2>/dev/null; then
  perl -e 'select(undef, undef, undef, $ARGV[0] / 1000)' "$FAKE_DELAY_MS"
fi

case "$FAKE_BEHAVIOR" in
  success)
    cat <<'NDJSON'
{"type":"system","subtype":"init","cwd":"/tmp/fake","session_id":"fake-session-1","tools":["Task","Bash","Edit","Read","WebFetch"],"model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.201","uuid":"f72f7f7c-89a4-473e-b60d-4ccbc2687100"}
{"type":"system","subtype":"thinking_tokens","estimated_tokens":8,"estimated_tokens_delta":8,"session_id":"fake-session-1"}
this line is not json and must be skipped without a panic
{"type":"warp_field_diagnostic","level":9,"session_id":"fake-session-1"}
{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_fake_01","type":"message","role":"assistant","content":[{"type":"text","text":"Engage."}]},"session_id":"fake-session-1","uuid":"6c5f16a4-37cc-4b4b-91ce-b3e8ca138301"}
{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_fake_02","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_fake_01","name":"Bash","input":{"command":"echo LCARS-OK","description":"Emit the ready marker"}}]},"session_id":"fake-session-1","uuid":"3c136da2-4d4e-4bed-b964-641eccfe6501"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_fake_01","type":"tool_result","content":"LCARS-OK","is_error":false}]},"session_id":"fake-session-1","uuid":"f847952c-03e6-4610-b546-0da97cd3df01"}
{"type":"result","subtype":"success","is_error":false,"api_error_status":null,"duration_ms":4364,"num_turns":2,"result":"DONE","stop_reason":"end_turn","session_id":"fake-session-1","total_cost_usd":0.0546649,"usage":{"input_tokens":16,"output_tokens":183,"cache_read_input_tokens":24899,"cache_creation_input_tokens":25072},"permission_denials":[],"terminal_reason":"completed","uuid":"dcd9dd1f-5723-47e2-9e1e-589c12e03201"}
NDJSON
    finish
    exit 0
    ;;
  nonzero)
    cat <<'NDJSON'
{"type":"system","subtype":"init","cwd":"/tmp/fake","session_id":"fake-session-1","tools":["Task","Bash","Edit","Read","WebFetch"],"model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.201","uuid":"f72f7f7c-89a4-473e-b60d-4ccbc2687101"}
NDJSON
    echo "warp core breach: simulated failure diagnostics" >&2
    finish
    exit 3
    ;;
  hang)
    sleep 30 </dev/null >/dev/null 2>&1
    finish
    exit 0
    ;;
  rate_limited)
    cat <<'NDJSON'
{"type":"system","subtype":"init","cwd":"/tmp/fake","session_id":"fake-session-1","tools":["Task","Bash","Edit","Read","WebFetch"],"model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.201","uuid":"f72f7f7c-89a4-473e-b60d-4ccbc2687102"}
{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1785542400,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false},"session_id":"fake-session-1","uuid":"82f099c7-eb08-45e1-91c0-62a873814901"}
{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":null,"duration_ms":812,"num_turns":0,"result":null,"session_id":"fake-session-1","total_cost_usd":0.0,"permission_denials":[],"terminal_reason":"error","uuid":"dcd9dd1f-5723-47e2-9e1e-589c12e03202"}
NDJSON
    finish
    exit 1
    ;;
  *)
    echo "unknown FAKE_BEHAVIOR: $FAKE_BEHAVIOR" >&2
    exit 64
    ;;
esac
"#;

#[derive(Debug, Clone, Default)]
pub struct FakeOpts {
    pub behavior: &'static str,
    pub delay_ms: u64,
    pub out: Option<PathBuf>,
    pub overlap_log: Option<PathBuf>,
}

impl FakeOpts {
    pub fn behavior(behavior: &'static str) -> Self {
        Self {
            behavior,
            ..Self::default()
        }
    }
}

/// Write the fake claude script into `dir` and make it executable.
pub fn write_fake_claude(dir: &Path, opts: &FakeOpts) -> PathBuf {
    let path_str = |p: &Option<PathBuf>| {
        p.as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let script = SCRIPT_TEMPLATE
        .replace("__BEHAVIOR__", opts.behavior)
        .replace("__DELAY_MS__", &opts.delay_ms.to_string())
        .replace("__OUT__", &path_str(&opts.out))
        .replace("__OVERLAP__", &path_str(&opts.overlap_log));
    let path = dir.join("fake_claude.sh");
    std::fs::write(&path, script).expect("write fake claude script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude script");
    path
}

pub fn runner(
    binary: &Path,
    max_concurrent: u32,
    kobayashi_reserved_slots: u32,
    turn_timeout_secs: u64,
) -> (ClaudeRunner, broadcast::Receiver<BridgeEvent>) {
    let (tx, rx) = broadcast::channel(4096);
    let config = ClaudeConfig {
        binary_path: binary.to_string_lossy().into_owned(),
        max_concurrent,
        kobayashi_reserved_slots,
        turn_timeout_secs,
        ..ClaudeConfig::default()
    };
    (ClaudeRunner::new(config, tx), rx)
}

pub fn invocation(cwd: &Path, prompt: &str) -> ClaudeInvocation {
    ClaudeInvocation {
        prompt: prompt.into(),
        cwd: cwd.to_path_buf(),
        max_turns: 5,
        ..ClaudeInvocation::default()
    }
}

pub fn turn_ctx(kobayashi: bool) -> TurnCtx {
    TurnCtx {
        workstream: WorkstreamId::new(),
        station: if kobayashi {
            Station::KobayashiMaru
        } else {
            Station::Helm
        },
        kobayashi,
        pid_register: None,
    }
}

pub fn drain_events(rx: &mut broadcast::Receiver<BridgeEvent>) -> Vec<BridgeEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

/// What the fake script dumped about one run.
#[derive(Debug)]
pub struct FakeRunDump {
    pub pid: u32,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

pub fn read_dump(path: &Path) -> FakeRunDump {
    let text = std::fs::read_to_string(path).expect("read FAKE_OUT dump");
    let mut pid = None;
    let mut args = Vec::new();
    let mut env = HashMap::new();
    let mut in_env = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PID=") {
            pid = rest.trim().parse().ok();
        } else if let Some(arg) = line.strip_prefix("ARG=") {
            args.push(arg.to_string());
        } else if line == "ENV_BEGIN" {
            in_env = true;
        } else if line == "ENV_END" {
            in_env = false;
        } else if in_env && let Some((key, value)) = line.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        }
    }
    FakeRunDump {
        pid: pid.expect("dump contains PID"),
        args,
        env,
    }
}

/// Maximum simultaneous runs recorded in an overlap journal, optionally
/// restricted to one tag ("general" / "kobayashi"). Correct because the
/// script appends its S/E markers with atomic O_APPEND writes, so file
/// order is interleaving order.
pub fn max_overlap(log_text: &str, tag_filter: Option<&str>) -> u32 {
    let mut current: i32 = 0;
    let mut max: i32 = 0;
    for line in log_text.lines() {
        let mut parts = line.split_whitespace();
        let (kind, tag) = (parts.next(), parts.next());
        if let Some(filter) = tag_filter
            && tag != Some(filter)
        {
            continue;
        }
        match kind {
            Some("S") => {
                current += 1;
                max = max.max(current);
            }
            Some("E") => current -= 1,
            _ => {}
        }
    }
    max.max(0) as u32
}

pub fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
