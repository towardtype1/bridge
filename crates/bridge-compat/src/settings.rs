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
    _server_url: &str,
    _workstream: WorkstreamId,
    _token: &str,
    _helper_path: &Path,
    _hook_timeout_secs: u32,
) -> serde_json::Value {
    todo!()
}
