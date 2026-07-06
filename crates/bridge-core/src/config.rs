//! Harness configuration, loaded from `bridge.toml`.
//!
//! Every field has a documented default so a missing section or key falls
//! back rather than failing. `bridge.example.toml` at the repo root mirrors
//! this structure with annotations.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// Top-level harness configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BridgeConfig {
    pub claude: ClaudeConfig,
    pub budgets: BudgetConfig,
    pub tactical: TacticalConfig,
    pub merge: MergeConfig,
    pub worktrees: WorktreeConfig,
    /// Linear MCP sync. Absent = disabled.
    pub linear: Option<LinearConfig>,
    #[serde(default)]
    pub ui: UiConfig,
}

impl BridgeConfig {
    /// Load and validate a config file. A missing file is an error; use
    /// `BridgeConfig::default()` when running without one.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        let cfg: BridgeConfig = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.claude.max_concurrent == 0 {
            return Err(ConfigError::Invalid(
                "claude.max_concurrent must be at least 1".into(),
            ));
        }
        if self.claude.kobayashi_reserved_slots >= self.claude.max_concurrent {
            return Err(ConfigError::Invalid(format!(
                "claude.kobayashi_reserved_slots ({}) must be less than claude.max_concurrent ({})",
                self.claude.kobayashi_reserved_slots, self.claude.max_concurrent
            )));
        }
        if self.budgets.max_turns_per_workstream == 0 || self.budgets.max_total_turns == 0 {
            return Err(ConfigError::Invalid(
                "budgets.max_total_turns and budgets.max_turns_per_workstream must be at least 1"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// How the Claude Code CLI is located and driven.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Path or command name of the claude binary.
    pub binary_path: String,
    /// Default model alias or full id for station turns.
    pub model: String,
    /// Model for Kobayashi Maru runs; default the strongest available tier.
    pub kobayashi_model: String,
    /// Inclusive lower bound of the tested CLI version range.
    pub tested_version_min: String,
    /// Inclusive upper bound of the tested CLI version range.
    pub tested_version_max: String,
    /// Maximum concurrent claude child processes (semaphore size).
    pub max_concurrent: u32,
    /// Slots of the semaphore reserved for Kobayashi Maru runs so testers
    /// and builders do not starve each other.
    pub kobayashi_reserved_slots: u32,
    /// Hard wall-clock timeout for a single claude invocation, seconds.
    pub turn_timeout_secs: u64,
    /// Timeout written into installed hook settings, seconds. Escalations
    /// must resolve inside this window; the control server answers deny at
    /// `tactical.escalation_timeout_secs`, which must be smaller.
    pub hook_timeout_secs: u32,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            binary_path: "claude".into(),
            model: "sonnet".into(),
            kobayashi_model: "fable".into(),
            tested_version_min: "2.1.190".into(),
            tested_version_max: "2.2.99".into(),
            max_concurrent: 3,
            kobayashi_reserved_slots: 1,
            turn_timeout_secs: 900,
            hook_timeout_secs: 600,
        }
    }
}

/// Per-mission budgets, enforced by Ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetConfig {
    pub max_total_turns: u32,
    pub max_turns_per_workstream: u32,
    pub max_kobayashi_rounds: u32,
    /// Optional wall-clock ceiling for the whole mission, seconds.
    pub max_wall_clock_secs: Option<u64>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_total_turns: 120,
            max_turns_per_workstream: 30,
            max_kobayashi_rounds: 3,
            max_wall_clock_secs: None,
        }
    }
}

/// Tactical policy knobs. Prime directives are hardcoded and not listed here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TacticalConfig {
    /// Regexes matched against Bash commands; a hit denies the call.
    pub destructive_patterns: Vec<String>,
    /// Regexes matched against file paths any tool touches; a hit denies.
    pub protected_path_patterns: Vec<String>,
    /// When false, network egress from Bash (curl/wget/nc/ssh) escalates.
    pub allow_network_egress: bool,
    /// One-shot cheap claude review of suspicious-but-not-denied calls.
    pub deep_scan: bool,
    /// How long an escalation waits for the user before failing closed to
    /// deny, seconds. Must be comfortably below `claude.hook_timeout_secs`.
    pub escalation_timeout_secs: u64,
}

impl Default for TacticalConfig {
    fn default() -> Self {
        Self {
            destructive_patterns: vec![
                r"rm\s+(-[a-zA-Z]*[rf][a-zA-Z]*\s+)+".into(),
                r"git\s+push\s+.*--force".into(),
                r"curl[^|]*\|\s*(ba)?sh".into(),
                r"wget[^|]*\|\s*(ba)?sh".into(),
                r"git\s+reset\s+--hard".into(),
                r"mkfs|diskutil\s+erase".into(),
            ],
            protected_path_patterns: vec![
                r"(^|/)\.ssh(/|$)".into(),
                r"(^|/)\.aws(/|$)".into(),
                r"(^|/)\.gnupg(/|$)".into(),
                r"(^|/)\.env(\.|$)".into(),
                r"(^|/)credentials(\.|$|/)".into(),
                r"(^|/)\.claude/settings\.json$".into(),
            ],
            allow_network_egress: false,
            deep_scan: false,
            escalation_timeout_secs: 540,
        }
    }
}

/// Merge policy for the serialized merge queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    pub mode: MergeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MergeMode {
    /// Rebase the workstream branch onto main, then fast-forward main.
    #[default]
    Rebase,
    /// Create a merge commit on main.
    MergeCommit,
}

/// Where worktrees live and what happens to them on mission end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorktreeConfig {
    /// Root directory for worktrees. Default: a per-repo directory under
    /// the platform data dir (resolved by the app, not here), never inside
    /// the target repository checkout.
    pub root: Option<PathBuf>,
    /// Keep worktrees and installed hook settings when a mission fails.
    pub keep_on_failure: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            root: None,
            keep_on_failure: true,
        }
    }
}

/// Linear MCP sync configuration. Presence enables the integration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearConfig {
    /// Path to an MCP config JSON passed via `--mcp-config`.
    pub mcp_config_path: PathBuf,
}

/// GUI preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Command used to open a worktree in an editor, e.g. "code", "cursor".
    pub editor_command: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            editor_command: "code".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let cfg = BridgeConfig::default();
        assert_eq!(cfg.claude.max_concurrent, 3);
        assert_eq!(cfg.budgets.max_kobayashi_rounds, 3);
        assert_eq!(cfg.merge.mode, MergeMode::Rebase);
        assert!(cfg.linear.is_none());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn empty_toml_gives_defaults() {
        let cfg: BridgeConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, BridgeConfig::default());
    }

    #[test]
    fn partial_section_keeps_other_defaults() {
        let cfg: BridgeConfig = toml::from_str("[claude]\nmax_concurrent = 5\n").unwrap();
        assert_eq!(cfg.claude.max_concurrent, 5);
        assert_eq!(cfg.claude.model, "sonnet");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = toml::from_str::<BridgeConfig>("[claude]\ntypo_key = 1\n");
        assert!(err.is_err(), "unknown keys should fail loudly, got {err:?}");
    }

    #[test]
    fn reserved_slots_must_leave_room() {
        let mut cfg = BridgeConfig::default();
        cfg.claude.kobayashi_reserved_slots = 3;
        assert!(matches!(cfg.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn load_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.toml");
        std::fs::write(&path, "[budgets]\nmax_total_turns = 7\n").unwrap();
        let cfg = BridgeConfig::load(&path).unwrap();
        assert_eq!(cfg.budgets.max_total_turns, 7);
    }

    #[test]
    fn load_missing_file_is_io_error() {
        let err = BridgeConfig::load(Path::new("/nonexistent/bridge.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn ui_editor_command_defaults_to_code() {
        let cfg = BridgeConfig::default();
        assert_eq!(cfg.ui.editor_command, "code");
        let parsed: BridgeConfig = toml::from_str("[ui]\neditor_command = \"cursor\"\n").unwrap();
        assert_eq!(parsed.ui.editor_command, "cursor");
    }
}
