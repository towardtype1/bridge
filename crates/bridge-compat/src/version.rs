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
    /// `"2.1.201 (Claude Code)"`, plus bare `"2.1.201"`. The first
    /// whitespace-separated token must be exactly `major.minor.patch`;
    /// anything after the first token is ignored as decoration.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || VersionParseError {
            input: s.to_owned(),
        };
        let token = s.split_whitespace().next().ok_or_else(err)?;
        let mut parts = token.split('.');
        let mut next_num = || -> Result<u32, VersionParseError> {
            parts.next().and_then(|p| p.parse().ok()).ok_or_else(err)
        };
        let (major, minor, patch) = (next_num()?, next_num()?, next_num()?);
        if parts.next().is_some() {
            return Err(err());
        }
        Ok(CliVersion {
            major,
            minor,
            patch,
        })
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
pub fn check_version(v: &CliVersion, cfg: &ClaudeConfig) -> VersionStatus {
    let min = parse_bound(&cfg.tested_version_min, "tested_version_min");
    let max = parse_bound(&cfg.tested_version_max, "tested_version_max");
    if let Some(min) = min
        && *v < min
    {
        return VersionStatus::OlderThanTested;
    }
    if let Some(max) = max
        && *v > max
    {
        return VersionStatus::NewerThanTested;
    }
    VersionStatus::Tested
}

fn parse_bound(raw: &str, which: &str) -> Option<CliVersion> {
    match raw.parse::<CliVersion>() {
        Ok(v) => Some(v),
        Err(_) => {
            tracing::warn!(
                bound = which,
                value = raw,
                "unparseable tested version bound; treating as open"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> CliVersion {
        CliVersion {
            major,
            minor,
            patch,
        }
    }

    fn cfg(min: &str, max: &str) -> ClaudeConfig {
        ClaudeConfig {
            tested_version_min: min.into(),
            tested_version_max: max.into(),
            ..ClaudeConfig::default()
        }
    }

    #[test]
    fn parses_the_real_version_banner() {
        let parsed: CliVersion = "2.1.201 (Claude Code)".parse().unwrap();
        assert_eq!(parsed, v(2, 1, 201));
    }

    #[test]
    fn parses_a_bare_triple() {
        let parsed: CliVersion = "2.1.201".parse().unwrap();
        assert_eq!(parsed, v(2, 1, 201));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let parsed: CliVersion = " 2.1.201\n".parse().unwrap();
        assert_eq!(parsed, v(2, 1, 201));
    }

    #[test]
    fn rejects_unparseable_input_with_the_input_preserved() {
        for input in ["", "banana", "2.1", "2.1.x", "2.1.201.7", "v2.1.201"] {
            let err = input.parse::<CliVersion>().unwrap_err();
            assert_eq!(err.input, input, "input echoed back for {input:?}");
        }
    }

    #[test]
    fn display_round_trips() {
        let parsed: CliVersion = v(2, 1, 201).to_string().parse().unwrap();
        assert_eq!(parsed, v(2, 1, 201));
    }

    #[test]
    fn ordering_is_numeric_not_lexicographic() {
        assert!(v(2, 1, 9) < v(2, 1, 10));
        assert!(v(2, 1, 201) < v(2, 2, 0));
        assert!(v(2, 10, 0) > v(2, 9, 99));
    }

    #[test]
    fn range_check_is_inclusive_on_both_ends() {
        let cfg = cfg("2.1.190", "2.2.99");
        assert_eq!(check_version(&v(2, 1, 190), &cfg), VersionStatus::Tested);
        assert_eq!(check_version(&v(2, 2, 99), &cfg), VersionStatus::Tested);
        assert_eq!(check_version(&v(2, 1, 201), &cfg), VersionStatus::Tested);
        assert_eq!(
            check_version(&v(2, 1, 189), &cfg),
            VersionStatus::OlderThanTested
        );
        assert_eq!(
            check_version(&v(2, 3, 0), &cfg),
            VersionStatus::NewerThanTested
        );
    }

    #[test]
    fn unparseable_min_bound_is_open_below() {
        let cfg = cfg("not-a-version", "2.2.99");
        assert_eq!(check_version(&v(0, 0, 1), &cfg), VersionStatus::Tested);
        assert_eq!(
            check_version(&v(9, 0, 0), &cfg),
            VersionStatus::NewerThanTested
        );
    }

    #[test]
    fn unparseable_max_bound_is_open_above() {
        let cfg = cfg("2.1.190", "");
        assert_eq!(check_version(&v(99, 0, 0), &cfg), VersionStatus::Tested);
        assert_eq!(
            check_version(&v(1, 0, 0), &cfg),
            VersionStatus::OlderThanTested
        );
    }

    #[test]
    fn both_bounds_unparseable_means_everything_is_tested() {
        let cfg = cfg("", "");
        assert_eq!(check_version(&v(0, 0, 0), &cfg), VersionStatus::Tested);
        assert_eq!(check_version(&v(99, 99, 99), &cfg), VersionStatus::Tested);
    }
}
