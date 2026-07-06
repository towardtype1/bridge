//! Kobayashi Maru battle reports.

use crate::ids::WorkstreamId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Clean,
    Breached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/// One failure the adversarial tester produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
    pub description: String,
    /// Command that reproduces the failure from the worktree root.
    pub reproduction_command: String,
    /// Path of the committed failing test, when one exists.
    pub failing_test_path: Option<String>,
    /// Recurring weakness taxonomy key, e.g. "path-traversal",
    /// "unvalidated-external-content", "race-condition". Used by the
    /// Ship's Computer so future runs attack known weak spots first.
    pub weakness_class: String,
}

/// Structured verdict of one Kobayashi Maru round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BattleReport {
    pub workstream: WorkstreamId,
    pub round: u32,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
}

impl BattleReport {
    /// JSON Schema handed to `claude --json-schema` for the tester's output.
    /// Deliberately excludes `workstream` and `round`; the harness stamps
    /// those, the model only reports verdict and findings.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "verdict": { "type": "string", "enum": ["clean", "breached"] },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "severity": { "type": "string", "enum": ["low", "medium", "high", "critical"] },
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "reproduction_command": { "type": "string" },
                            "failing_test_path": { "type": ["string", "null"] },
                            "weakness_class": { "type": "string" }
                        },
                        "required": ["severity", "title", "description", "reproduction_command", "weakness_class"]
                    }
                }
            },
            "required": ["verdict", "findings"]
        })
    }

    /// Parse the model's structured output and stamp harness context.
    pub fn from_model_output(
        value: &serde_json::Value,
        workstream: WorkstreamId,
        round: u32,
    ) -> Result<Self, serde_json::Error> {
        #[derive(Deserialize)]
        struct ModelReport {
            verdict: Verdict,
            findings: Vec<Finding>,
        }
        let m: ModelReport = serde_json::from_value(value.clone())?;
        Ok(Self {
            workstream,
            round,
            verdict: m.verdict,
            findings: m.findings,
        })
    }

    /// A breached verdict with zero findings (or vice versa) is inconsistent.
    pub fn is_consistent(&self) -> bool {
        match self.verdict {
            Verdict::Clean => self.findings.is_empty(),
            Verdict::Breached => !self.findings.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_output_parses_and_gets_stamped() {
        let raw = serde_json::json!({
            "verdict": "breached",
            "findings": [{
                "severity": "high",
                "title": "path traversal in loader",
                "description": "loader joins user input onto base path without normalization",
                "reproduction_command": "cargo test --test adversarial_loader",
                "failing_test_path": "tests/adversarial/loader_traversal.rs",
                "weakness_class": "path-traversal"
            }]
        });
        let ws = WorkstreamId::new();
        let report = BattleReport::from_model_output(&raw, ws, 2).unwrap();
        assert_eq!(report.workstream, ws);
        assert_eq!(report.round, 2);
        assert_eq!(report.verdict, Verdict::Breached);
        assert_eq!(report.findings[0].severity, Severity::High);
        assert!(report.is_consistent());
    }

    #[test]
    fn missing_optional_test_path_is_fine() {
        let raw = serde_json::json!({
            "verdict": "breached",
            "findings": [{
                "severity": "low",
                "title": "t",
                "description": "d",
                "reproduction_command": "true",
                "weakness_class": "misc"
            }]
        });
        let report = BattleReport::from_model_output(&raw, WorkstreamId::new(), 1).unwrap();
        assert_eq!(report.findings[0].failing_test_path, None);
    }

    #[test]
    fn inconsistent_reports_are_detected() {
        let raw = serde_json::json!({ "verdict": "breached", "findings": [] });
        let report = BattleReport::from_model_output(&raw, WorkstreamId::new(), 1).unwrap();
        assert!(!report.is_consistent());
    }

    #[test]
    fn severity_orders_for_sorting() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
    }
}
