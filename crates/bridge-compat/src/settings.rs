//! Rendering of the `.claude/settings.json` Engineering installs into each
//! worktree: PreToolUse, PostToolUse and Stop hooks that run the bundled
//! helper binary with the control server coordinates in env.

use bridge_core::WorkstreamId;
use std::path::Path;

/// Build the settings.json document for one worktree.
///
/// Shape (verified working on 2.1.201 via `--setting-sources project`):
/// each of PreToolUse/PostToolUse/Stop gets one matcher-`"*"` entry with a
/// single SHELL-FORM command hook (command hooks have no documented env
/// map, so env rides as assignment prefixes):
///
/// ```json
/// { "type": "command",
///   "command": "BRIDGE_SERVER_URL='<url>' BRIDGE_WORKSTREAM_ID='<id>' BRIDGE_TOKEN='<token>' '<helper_path>'",
///   "timeout": <hook_timeout_secs> }
/// ```
///
/// Values are single-quoted with embedded single quotes escaped via the
/// standard `'\''` idiom; the helper path may contain spaces.
pub fn render_worktree_settings(
    server_url: &str,
    workstream: WorkstreamId,
    token: &str,
    helper_path: &Path,
    hook_timeout_secs: u32,
) -> serde_json::Value {
    let command = format!(
        "{}={} {}={} {}={} {}",
        bridge_core::wire::ENV_SERVER_URL,
        shell_single_quote(server_url),
        bridge_core::wire::ENV_WORKSTREAM_ID,
        shell_single_quote(&workstream.to_string()),
        bridge_core::wire::ENV_TOKEN,
        shell_single_quote(token),
        shell_single_quote(&helper_path.to_string_lossy()),
    );
    let entry = serde_json::json!([{
        "matcher": "*",
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": hook_timeout_secs,
        }]
    }]);
    serde_json::json!({
        "hooks": {
            "PreToolUse": entry.clone(),
            "PostToolUse": entry.clone(),
            "Stop": entry,
        }
    })
}

/// Single-quote a string for POSIX shells: wrap in `'...'` and escape any
/// embedded single quote with the standard `'\''` idiom.
fn shell_single_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WS: &str = "0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0";

    #[test]
    fn golden_settings_document() {
        let ws: WorkstreamId = WS.parse().unwrap();
        let doc = render_worktree_settings(
            "http://127.0.0.1:8471",
            ws,
            "secret-token",
            Path::new("/opt/bridge/bridge-hook-helper"),
            600,
        );
        let command = format!(
            "BRIDGE_SERVER_URL='http://127.0.0.1:8471' BRIDGE_WORKSTREAM_ID='{WS}' \
             BRIDGE_TOKEN='secret-token' '/opt/bridge/bridge-hook-helper'"
        );
        let entry = json!([{
            "matcher": "*",
            "hooks": [{"type": "command", "command": command, "timeout": 600}]
        }]);
        assert_eq!(
            doc,
            json!({
                "hooks": {
                    "PreToolUse": entry,
                    "PostToolUse": entry,
                    "Stop": entry,
                }
            })
        );
    }

    #[test]
    fn embedded_single_quotes_and_spaces_are_escaped_shell_safely() {
        let ws: WorkstreamId = WS.parse().unwrap();
        let doc = render_worktree_settings(
            "http://127.0.0.1:1'1",
            ws,
            "tok'en",
            Path::new("/opt/bridge tools/o'brien-helper"),
            30,
        );
        let command = doc["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command string");
        assert_eq!(
            command,
            format!(
                "BRIDGE_SERVER_URL='http://127.0.0.1:1'\\''1' BRIDGE_WORKSTREAM_ID='{WS}' \
                 BRIDGE_TOKEN='tok'\\''en' '/opt/bridge tools/o'\\''brien-helper'"
            )
        );
    }

    #[test]
    fn all_three_events_are_covered_with_star_matcher_and_timeout() {
        let ws: WorkstreamId = WS.parse().unwrap();
        let doc =
            render_worktree_settings("http://127.0.0.1:9", ws, "t", Path::new("/bin/helper"), 42);
        for event in ["PreToolUse", "PostToolUse", "Stop"] {
            let entries = doc["hooks"][event].as_array().unwrap_or_else(|| {
                panic!("{event} must be present as an array");
            });
            assert_eq!(entries.len(), 1, "{event}: one matcher entry");
            assert_eq!(entries[0]["matcher"], "*", "{event}: matcher");
            let hooks = entries[0]["hooks"].as_array().expect("hooks array");
            assert_eq!(hooks.len(), 1, "{event}: one hook");
            assert_eq!(hooks[0]["type"], "command", "{event}: type");
            assert_eq!(hooks[0]["timeout"], 42, "{event}: timeout");
        }
    }
}
