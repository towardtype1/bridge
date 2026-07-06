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
    /// - `--allowedTools` / `--disallowedTools`: the flag once, then ONE
    ///   argv element per rule (rules may contain spaces, e.g.
    ///   `Bash(echo *)`; no joining, no shell quoting, safe through
    ///   `std::process::Command`), only when non-empty
    /// - `--model`, `--mcp-config` (+ `--strict-mcp-config`),
    ///   `--append-system-prompt`, `--json-schema` (compact JSON as one
    ///   arg), `--setting-sources` (comma-joined as one arg) only when set
    ///
    /// Fixed argument order (asserted by golden tests): `-p`, `--verbose`,
    /// `--output-format`, `--max-turns`, `--resume`, `--model`,
    /// `--allowedTools`, `--disallowedTools`, `--append-system-prompt`,
    /// `--json-schema`, `--mcp-config`, `--strict-mcp-config`,
    /// `--setting-sources`.
    pub fn to_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "-p".into(),
            self.prompt.as_str().into(),
            "--verbose".into(),
            "--output-format".into(),
            match self.output_format {
                OutputFormat::StreamJson => "stream-json".into(),
                OutputFormat::Json => "json".into(),
            },
            "--max-turns".into(),
            self.max_turns.to_string().into(),
        ];
        if let Some(session) = &self.resume {
            args.push("--resume".into());
            args.push(session.0.as_str().into());
        }
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.as_str().into());
        }
        if !self.allowed_tools.is_empty() {
            args.push("--allowedTools".into());
            args.extend(self.allowed_tools.iter().map(|rule| rule.as_str().into()));
        }
        if !self.disallowed_tools.is_empty() {
            args.push("--disallowedTools".into());
            args.extend(
                self.disallowed_tools
                    .iter()
                    .map(|rule| rule.as_str().into()),
            );
        }
        if let Some(system_prompt) = &self.append_system_prompt {
            args.push("--append-system-prompt".into());
            args.push(system_prompt.as_str().into());
        }
        if let Some(schema) = &self.json_schema {
            args.push("--json-schema".into());
            args.push(
                serde_json::to_string(schema)
                    .expect("serde_json::Value serialization cannot fail")
                    .into(),
            );
        }
        if let Some(mcp_config) = &self.mcp_config {
            args.push("--mcp-config".into());
            args.push(mcp_config.as_os_str().to_owned());
            args.push("--strict-mcp-config".into());
        }
        if !self.setting_sources.is_empty() {
            args.push("--setting-sources".into());
            args.push(self.setting_sources.join(",").into());
        }
        args
    }
}

/// Env vars that must be REMOVED from every child claude process:
/// `ANTHROPIC_API_KEY` (would override subscription auth) and the
/// session-identity vars a parent Claude Code session leaks
/// (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT`,
/// `CLAUDE_CODE_CHILD_SESSION`).
pub fn scrubbed_env() -> &'static [&'static str] {
    &[
        "ANTHROPIC_API_KEY",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_CHILD_SESSION",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(strs: &[&str]) -> Vec<OsString> {
        strs.iter().map(OsString::from).collect()
    }

    #[test]
    fn golden_minimal_turn() {
        let inv = ClaudeInvocation {
            prompt: "Report status".into(),
            cwd: PathBuf::from("/tmp/wt"),
            max_turns: 4,
            ..ClaudeInvocation::default()
        };
        assert_eq!(
            inv.to_args(),
            args_of(&[
                "-p",
                "Report status",
                "--verbose",
                "--output-format",
                "stream-json",
                "--max-turns",
                "4",
            ])
        );
    }

    #[test]
    fn golden_full_invocation() {
        let inv = ClaudeInvocation {
            prompt: "Fix the bug".into(),
            cwd: PathBuf::from("/tmp/wt"),
            resume: Some(SessionId::from("a2356b0f-c205-48cc-8654-88c062868484")),
            max_turns: 8,
            allowed_tools: vec!["Bash(echo *)".into(), "Read".into()],
            disallowed_tools: vec!["WebSearch".into()],
            model: Some("sonnet".into()),
            json_schema: Some(serde_json::json!({"type": "object"})),
            mcp_config: Some(PathBuf::from("/tmp/wt/.bridge/mcp.json")),
            append_system_prompt: Some("You are Helm.".into()),
            output_format: OutputFormat::Json,
            setting_sources: vec!["project".into(), "local".into()],
        };
        assert_eq!(
            inv.to_args(),
            args_of(&[
                "-p",
                "Fix the bug",
                "--verbose",
                "--output-format",
                "json",
                "--max-turns",
                "8",
                "--resume",
                "a2356b0f-c205-48cc-8654-88c062868484",
                "--model",
                "sonnet",
                "--allowedTools",
                "Bash(echo *)",
                "Read",
                "--disallowedTools",
                "WebSearch",
                "--append-system-prompt",
                "You are Helm.",
                "--json-schema",
                r#"{"type":"object"}"#,
                "--mcp-config",
                "/tmp/wt/.bridge/mcp.json",
                "--strict-mcp-config",
                "--setting-sources",
                "project,local",
            ])
        );
    }

    #[test]
    fn never_emits_bare() {
        let inv = ClaudeInvocation {
            prompt: "x".into(),
            max_turns: 1,
            ..ClaudeInvocation::default()
        };
        assert!(!inv.to_args().contains(&OsString::from("--bare")));
    }

    #[test]
    fn empty_tool_lists_omit_the_flags() {
        let inv = ClaudeInvocation {
            prompt: "x".into(),
            max_turns: 1,
            ..ClaudeInvocation::default()
        };
        let args = inv.to_args();
        assert!(!args.contains(&OsString::from("--allowedTools")));
        assert!(!args.contains(&OsString::from("--disallowedTools")));
        assert!(!args.contains(&OsString::from("--setting-sources")));
        assert!(!args.contains(&OsString::from("--resume")));
    }

    #[test]
    fn scrub_list_covers_api_key_and_session_identity_vars() {
        let vars = scrubbed_env();
        for expected in [
            "ANTHROPIC_API_KEY",
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_CHILD_SESSION",
        ] {
            assert!(vars.contains(&expected), "missing {expected}");
        }
        assert_eq!(vars.len(), 5);
    }
}
