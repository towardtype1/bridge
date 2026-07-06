//! Startup preflight: version, auth mode, tool inventory.

use bridge_compat::CliVersion;
use bridge_core::ClaudeConfig;

use crate::runner::EngineError;

#[derive(Debug, Clone, PartialEq)]
pub struct PreflightReport {
    pub version: CliVersion,
    pub raw_version: String,
    /// From `bridge_compat::check_version`.
    pub version_tested: bool,
    /// Tool names the CLI reports (from a probe run's system/init event),
    /// used to validate station allowlists. Empty when the probe was
    /// skipped.
    pub available_tools: Vec<String>,
}

/// Run `claude --version` (cheap, no model call) and parse it. Does NOT
/// probe a full turn by default; `probe_tools` additionally runs a
/// max-turns-1 haiku no-op to capture the init event's tool list.
pub async fn preflight(config: &ClaudeConfig, probe_tools: bool) -> Result<PreflightReport, EngineError> {
    todo!()
}
