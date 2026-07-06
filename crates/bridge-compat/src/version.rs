//! CLI version detection and tested-range checks.

use bridge_core::ClaudeConfig;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
#[error("cannot parse claude version from {input:?}")]
pub struct VersionParseError {
    pub input: String,
}

/// Semantic-ish version of the claude CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CliVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl std::str::FromStr for CliVersion {
    type Err = VersionParseError;

    /// Accepts the raw output of `claude --version`, e.g.
    /// `"2.1.201 (Claude Code)"`, plus bare `"2.1.201"`.
    fn from_str(_s: &str) -> Result<Self, Self::Err> {
        todo!("parse leading dotted triple, ignore trailing decoration")
    }
}

impl std::fmt::Display for CliVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStatus {
    Tested,
    OlderThanTested,
    NewerThanTested,
}

/// Compare a detected version against the inclusive tested range declared
/// in config (`tested_version_min` ..= `tested_version_max`). Unparseable
/// config bounds are treated as an open bound on that side (log a warning).
pub fn check_version(_v: &CliVersion, _cfg: &ClaudeConfig) -> VersionStatus {
    todo!()
}
