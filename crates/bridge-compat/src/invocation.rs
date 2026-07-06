//! Claude CLI invocation construction: the ONLY place flags are spelled.

use bridge_core::SessionId;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    StreamJson,
    Json,
}

/// Everything needed to spawn one `claude -p` turn. `to_args()` renders
/// the argv AFTER the binary name. `cwd` is carried for the spawner; it is
/// not an argument.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeInvocation {
    pub prompt: String,
    pub cwd: PathBuf,
    pub resume: Option<SessionId>,
    pub max_turns: u32,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub model: Option<String>,
    pub json_schema: Option<serde_json::Value>,
    pub mcp_config: Option<PathBuf>,
    pub append_system_prompt: Option<String>,
    pub output_format: OutputFormat,
    /// Restrict which setting sources load, e.g. ["project", "local"].
    /// Empty = flag omitted (CLI default: all sources).
    pub setting_sources: Vec<String>,
}

impl ClaudeInvocation {
    /// Render argv. Rules (verified 2.1.201):
    /// - always: `-p <prompt> --verbose --max-turns <n>`
    /// - `--output-format stream-json` or `json` per `output_format`
    /// - NEVER `--bare` (breaks subscription OAuth)
    /// - `--resume <id>` when resuming (same worktree cwd required)
    /// - `--allowedTools` / `--disallowedTools` as space-separated repeated
    ///   args, only when non-empty
    /// - `--model`, `--mcp-config` (+ `--strict-mcp-config`),
    ///   `--append-system-prompt`, `--json-schema`, `--setting-sources`
    ///   only when set
    pub fn to_args(&self) -> Vec<OsString> {
        todo!()
    }
}

/// Env vars that must be REMOVED from every child claude process:
/// `ANTHROPIC_API_KEY` (would override subscription auth) and the
/// session-identity vars a parent Claude Code session leaks
/// (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT`,
/// `CLAUDE_CODE_CHILD_SESSION`).
pub fn scrubbed_env() -> &'static [&'static str] {
    todo!()
}
