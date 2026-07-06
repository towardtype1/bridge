//! Prompt templates for station agents. Pure functions of their inputs so
//! they are trivially testable and reviewable in one place.

use bridge_core::{Finding, MissionPlan, WorkstreamSpec};

/// Captain planning prompt: decompose `objective` into parallelizable
/// workstreams with dependencies, kebab-case slugs, self-contained
/// descriptions (the Helm agent sees ONLY its description). Instructs the
/// model that output must satisfy the PlanDraft JSON schema.
pub fn captain_plan(objective: &str, repo_summary: &str) -> String {
    todo!()
}

/// Helm execution prompt for one workstream brief.
pub fn helm_order(ws: &WorkstreamSpec, objective: &str) -> String {
    todo!()
}

/// Helm fix prompt after a breach: includes the findings with repro
/// commands and the committed adversarial test paths.
pub fn helm_fix(ws: &WorkstreamSpec, findings: &[Finding]) -> String {
    todo!()
}

/// Helm rebase-conflict resolution prompt.
pub fn helm_resolve_conflicts(ws: &WorkstreamSpec, files: &[String]) -> String {
    todo!()
}

/// Kobayashi Maru prompt. States the goal plainly: break this change;
/// credit for finding failures, not confirming success. Lists the diff
/// summary, prior findings for in-scope files (recurring weakness classes
/// first), the attack surface checklist from the spec, and the output
/// contract (BattleReport JSON schema; commit failing tests under
/// tests/adversarial/).
pub fn kobayashi_attack(
    ws: &WorkstreamSpec,
    diff_stat: &str,
    prior_findings: &[Finding],
) -> String {
    todo!()
}

/// Comms final-answer prompt: given the mission objective, per-workstream
/// outcomes and merge results, produce the user-facing mission report.
pub fn comms_mission_report(plan: &MissionPlan, outcomes_summary: &str) -> String {
    todo!()
}
