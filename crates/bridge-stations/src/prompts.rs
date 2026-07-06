//! Prompt templates for station agents. Pure functions of their inputs so
//! they are trivially testable and reviewable in one place.

use bridge_core::{Finding, MissionPlan, WorkstreamSpec};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Helm execution prompt for one workstream brief.
pub fn helm_order(ws: &WorkstreamSpec, objective: &str) -> String {
    format!(
        "You are executing one workstream of a larger mission.\n\
         \n\
         ## Mission objective (context only)\n\
         {objective}\n\
         \n\
         ## Your workstream: {title}\n\
         {description}\n\
         \n\
         ## Rules\n\
         - Work only inside this worktree (branch {branch}); never touch files outside it.\n\
         - Write tests for what you build and make the existing test suite pass.\n\
         - Commit your work in small, clearly-described commits.\n\
         - Do not push to any remote; merging is handled for you.",
        title = ws.title,
        description = ws.description,
        branch = ws.slug,
    )
}

/// Helm fix prompt after a breach: includes the findings with repro
/// commands and the committed adversarial test paths.
pub fn helm_fix(ws: &WorkstreamSpec, findings: &[Finding]) -> String {
    let mut out = format!(
        "Adversarial testing BREACHED your workstream \"{title}\". Fix every finding \
         below, keep the adversarial tests passing, and commit the fixes in this \
         worktree.\n\n## Findings\n",
        title = ws.title,
    );
    for (i, f) in findings.iter().enumerate() {
        let _ = writeln!(
            out,
            "{n}. [{severity:?}] {title} (weakness class: {class})\n   {description}\n   Reproduce: {repro}",
            n = i + 1,
            severity = f.severity,
            title = f.title,
            class = f.weakness_class,
            description = f.description,
            repro = f.reproduction_command,
        );
        if let Some(path) = &f.failing_test_path {
            let _ = writeln!(out, "   Failing test committed at: {path}");
        }
    }
    out.push_str(
        "\n## Rules\n\
         - The committed failing tests under tests/adversarial/ are the acceptance \
           criteria: make them pass without weakening them.\n\
         - Fix root causes, not symptoms. Do not delete or skip adversarial tests.",
    );
    out
}

/// Helm rebase-conflict resolution prompt.
pub fn helm_resolve_conflicts(ws: &WorkstreamSpec, files: &[String]) -> String {
    let mut out = format!(
        "Your workstream \"{title}\" no longer rebases cleanly onto the updated main \
         branch. Rebase this worktree's branch onto main and resolve every conflict, \
         preserving both your changes' intent and the intent of what merged to main.\n\
         \n## Conflicting files\n",
        title = ws.title,
    );
    for f in files {
        let _ = writeln!(out, "- {f}");
    }
    out.push_str(
        "\n## Rules\n\
         - Finish the rebase so the branch history is clean on top of main.\n\
         - Run the tests after resolving; fix fallout caused by the rebase.\n\
         - Do not push to any remote.",
    );
    out
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
    let mut out = format!(
        "You are the adversarial tester. Your goal is to BREAK the change under test. \
         You get credit for finding real failures, not for confirming success. Assume \
         the implementation is wrong and hunt for the proof.\n\
         \n\
         ## Change under test: {title}\n\
         {description}\n\
         \n\
         ## Diff summary\n\
         {diff_stat}\n",
        title = ws.title,
        description = ws.description,
    );

    out.push_str("\n## Prior findings for the touched files (recurring weakness classes first)\n");
    if prior_findings.is_empty() {
        out.push_str("None recorded. This code has not been breached before; be the first.\n");
    } else {
        for f in order_by_recurrence(prior_findings) {
            let _ = writeln!(
                out,
                "- [{severity:?}] ({class}) {title}: {description}",
                severity = f.severity,
                class = f.weakness_class,
                title = f.title,
                description = f.description,
            );
        }
    }

    out.push_str(
        "\n## Attack surface checklist\n\
         - Boundary values, empty and huge inputs, unicode and path edge cases.\n\
         - Error paths: injected failures, partial writes, missing files, bad permissions.\n\
         - Concurrency: races, ordering assumptions, shared state.\n\
         - Contract violations: does the code do what its own docs and types promise?\n\
         - Malformed and hostile external content (never a reason to relax parsing).\n\
         - Recurring weakness classes listed above FIRST: they broke before.\n\
         \n\
         ## Rules\n\
         - You may create and edit files ONLY under tests/adversarial/. You can never \
           modify the implementation you are attacking.\n\
         - Every failure you claim must have a committed failing test under \
           tests/adversarial/ plus the exact command that reproduces it.\n\
         - Commit your adversarial tests before finishing.\n\
         \n\
         ## Output contract\n\
         Your final output must be JSON satisfying the BattleReport schema: \
         {\"verdict\": \"clean\" | \"breached\", \"findings\": [{\"severity\", \"title\", \
         \"description\", \"reproduction_command\", \"failing_test_path\", \
         \"weakness_class\"}]}. Report \"breached\" with findings when anything failed; \
         report \"clean\" with an empty findings list ONLY when you genuinely could not \
         break it.",
    );
    out
}

/// Order findings so recurring weakness classes come first: classes sorted
/// by descending occurrence count (ties by first appearance), findings
/// within a class keeping their given (newest-first) order.
fn order_by_recurrence(findings: &[Finding]) -> Vec<&Finding> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut first_seen: HashMap<&str, usize> = HashMap::new();
    for (i, f) in findings.iter().enumerate() {
        *counts.entry(f.weakness_class.as_str()).or_default() += 1;
        first_seen.entry(f.weakness_class.as_str()).or_insert(i);
    }
    let mut classes: Vec<&str> = counts.keys().copied().collect();
    classes.sort_by_key(|c| (std::cmp::Reverse(counts[c]), first_seen[c]));
    let mut out = Vec::with_capacity(findings.len());
    for class in classes {
        out.extend(findings.iter().filter(|f| f.weakness_class == class));
    }
    out
}

/// Comms final-answer prompt: given the mission objective, per-workstream
/// outcomes and merge results, produce the user-facing mission report.
pub fn comms_mission_report(plan: &MissionPlan, outcomes_summary: &str) -> String {
    let mut out = format!(
        "Write the final mission report for the user.\n\
         \n\
         ## Mission objective\n\
         {objective}\n\
         \n\
         ## Workstreams\n",
        objective = plan.objective,
    );
    for ws in &plan.workstreams {
        let _ = writeln!(out, "- {slug}: {title}", slug = ws.slug, title = ws.title);
    }
    let _ = write!(
        out,
        "\n## Outcomes\n{outcomes_summary}\n\
         \n\
         ## Instructions\n\
         Summarize honestly what was accomplished, what merged, what failed or was \
         flagged, and what the user should do next. Report only what the outcomes above \
         support; never invent results."
    );
    out
}

/// The shared conversational contract paragraph used by both captain_confer_opening
/// and captain_recap, ensuring the wording cannot drift between them.
const CONTRACT: &str = "Every reply must satisfy the CaptainReply JSON schema: converse in \
     \"message\" (ask clarifying questions, explain trade-offs, push back), and \
     include \"proposed_plan\" only when you are ready to propose. Nothing \
     launches until your officer explicitly approves a proposal, so do not \
     rush to one if the objective is unclear. When amending later, always \
     re-emit the full desired plan, not a delta.";

/// Error message prefix when a plan is rejected.
pub const PLAN_REJECTED_PREFIX: &str = "PLAN REJECTED: ";

/// Error message prefix when an amendment is rejected.
pub const AMENDMENT_REJECTED_PREFIX: &str = "AMENDMENT REJECTED: ";

/// Message sent when the captain's reply does not match the CaptainReply schema.
pub const SCHEMA_RETRY_MSG: &str = "Your last reply did not match the required CaptainReply schema. Reply again: converse in \"message\"; include \"proposed_plan\" only when proposing a plan.";

/// Opening turn of a Captain conference. The Captain converses in
/// `message` and attaches `proposed_plan` only when ready; every proposal
/// re-emits the FULL desired plan (the controller computes diffs).
pub fn captain_confer_opening(objective: &str, repo_summary: &str) -> String {
    format!(
        "You are opening a planning conference with your commanding officer.\n\
         \n\
         ## Objective\n\
         {objective}\n\
         \n\
         ## Repository\n\
         {repo_summary}\n\
         \n\
         ## How this conversation works\n\
         {contract}\n\
         \n\
         ## Plan rules (when you do propose)\n\
         - Each workstream gets a short kebab-case slug (lowercase alphanumerics and \
           hyphens; it becomes part of a git branch name), a title, and a description.\n\
         - Descriptions must be fully self-contained working briefs: the executing agent \
           sees ONLY its own description, never the objective, the other workstreams, or \
           this conversation. Include every file path, constraint and acceptance \
           criterion it needs.\n\
         - Prefer independent workstreams. Only add a depends_on entry (by slug) when one \
           workstream genuinely cannot start before another has merged.\n\
         - The dependency graph must be acyclic.",
        contract = CONTRACT,
    )
}

/// Fresh-session fallback prompt: recap the objective, list current workstream
/// statuses, and restate the conversational contract.
pub fn captain_recap(objective: &str, plan: &MissionPlan, statuses: &[(String, String)]) -> String {
    let mut out = format!(
        "You are resuming a planning conference with your commanding officer.\n\
         \n\
         ## Objective\n\
         {objective}\n\
         \n\
         ## Current workstreams\n",
        objective = objective,
    );
    // Build a lookup map for status by slug
    let status_map: std::collections::HashMap<&str, &str> = statuses
        .iter()
        .map(|(slug, status)| (slug.as_str(), status.as_str()))
        .collect();

    // Iterate over all workstreams so every one appears in the recap
    for ws in &plan.workstreams {
        let status = status_map
            .get(ws.slug.as_str())
            .copied()
            .unwrap_or("no status yet");
        let _ = writeln!(out, "- {} ({}): {}", ws.slug, ws.title, status);
    }
    let _ = write!(
        out,
        "\n## How this conversation works\n\
         {contract}",
        contract = CONTRACT,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::{MissionId, PlanDraft, PlanDraftWorkstream, Severity, WorkstreamId};

    #[test]
    fn captain_confer_opening_carries_contract() {
        let p = captain_confer_opening("Ship the frobnicator", "main branch: main");
        assert!(p.contains("Ship the frobnicator"));
        assert!(p.contains("main branch: main"));
        assert!(p.contains("proposed_plan"));
        assert!(p.contains("kebab-case"));
        assert!(p.contains("self-contained"));
        assert!(p.contains("full desired plan"), "amendment contract");
    }

    #[test]
    fn captain_recap_lists_workstreams_and_statuses() {
        let draft = PlanDraft {
            workstreams: vec![PlanDraftWorkstream {
                slug: "part-a".into(),
                title: "Part A".into(),
                description: "d".into(),
                depends_on: vec![],
            }],
        };
        let plan = MissionPlan::from_draft(draft, MissionId::new(), "m", "obj", "main").unwrap();
        let p = captain_recap("obj", &plan, &[("part-a".into(), "Working".into())]);
        assert!(p.contains("part-a"));
        assert!(p.contains("Working"));
        assert!(p.contains("obj"));
    }

    #[test]
    fn captain_recap_includes_all_workstreams_even_without_status() {
        let draft = PlanDraft {
            workstreams: vec![
                PlanDraftWorkstream {
                    slug: "first-workstream".into(),
                    title: "First Task".into(),
                    description: "d1".into(),
                    depends_on: vec![],
                },
                PlanDraftWorkstream {
                    slug: "second-workstream".into(),
                    title: "Second Task".into(),
                    description: "d2".into(),
                    depends_on: vec![],
                },
            ],
        };
        let plan = MissionPlan::from_draft(draft, MissionId::new(), "m", "obj", "main").unwrap();
        // Only provide status for the first workstream
        let p = captain_recap(
            "obj",
            &plan,
            &[("first-workstream".into(), "In Progress".into())],
        );
        // Both workstreams must appear in the recap
        assert!(p.contains("first-workstream (First Task): In Progress"));
        assert!(
            p.contains("second-workstream (Second Task): no status yet"),
            "missing workstream must show 'no status yet'"
        );
    }

    #[test]
    fn constant_values_are_pinned() {
        assert_eq!(PLAN_REJECTED_PREFIX, "PLAN REJECTED: ");
        assert_eq!(AMENDMENT_REJECTED_PREFIX, "AMENDMENT REJECTED: ");
        assert!(
            SCHEMA_RETRY_MSG.contains("CaptainReply"),
            "SCHEMA_RETRY_MSG must mention CaptainReply"
        );
        assert!(
            SCHEMA_RETRY_MSG.contains("proposed_plan"),
            "SCHEMA_RETRY_MSG must mention proposed_plan"
        );
    }

    fn spec() -> WorkstreamSpec {
        WorkstreamSpec {
            id: WorkstreamId::new(),
            slug: "fix-auth-timeout".into(),
            title: "Fix auth timeout".into(),
            description: "Sessions expire after 5 minutes because of a hardcoded TTL.".into(),
            base_ref: "main".into(),
        }
    }

    fn finding(class: &str, title: &str) -> Finding {
        Finding {
            severity: Severity::High,
            title: title.into(),
            description: format!("description of {title}"),
            reproduction_command: format!("cargo test --test {class}"),
            failing_test_path: Some(format!("tests/adversarial/{class}.rs")),
            weakness_class: class.into(),
        }
    }

    #[test]
    fn helm_order_carries_brief_verbatim() {
        let ws = spec();
        let p = helm_order(&ws, "Overall mission text");
        assert!(p.contains("Fix auth timeout"));
        assert!(p.contains("Sessions expire after 5 minutes because of a hardcoded TTL."));
        assert!(p.contains("Overall mission text"));
        assert!(p.contains("Do not push"));
    }

    #[test]
    fn helm_fix_lists_findings_with_repro_and_test_paths() {
        let ws = spec();
        let f1 = finding("path-traversal", "loader escapes base dir");
        let mut f2 = finding("race-condition", "double flush");
        f2.failing_test_path = None;
        let p = helm_fix(&ws, &[f1.clone(), f2.clone()]);
        assert!(p.contains("loader escapes base dir"));
        assert!(p.contains(&f1.reproduction_command));
        assert!(p.contains("tests/adversarial/path-traversal.rs"));
        assert!(p.contains("double flush"));
        assert!(p.contains(&f2.reproduction_command));
        assert!(p.contains("tests/adversarial/"));
    }

    #[test]
    fn conflict_prompt_lists_every_file() {
        let ws = spec();
        let files = vec!["src/auth.rs".to_string(), "src/session/ttl.rs".to_string()];
        let p = helm_resolve_conflicts(&ws, &files);
        assert!(p.contains("src/auth.rs"));
        assert!(p.contains("src/session/ttl.rs"));
        assert!(p.contains("rebase"));
    }

    #[test]
    fn kobayashi_states_goal_restriction_and_contract() {
        let ws = spec();
        let p = kobayashi_attack(&ws, " 3 files changed", &[]);
        assert!(p.contains("BREAK"));
        assert!(p.contains("finding real failures, not for confirming success"));
        assert!(p.contains("ONLY under tests/adversarial/"));
        assert!(p.contains("BattleReport"));
        assert!(p.contains("\"verdict\""));
        assert!(p.contains("\"breached\""));
        assert!(p.contains(" 3 files changed"));
        assert!(p.contains(&ws.description));
    }

    #[test]
    fn kobayashi_orders_prior_findings_by_recurrence() {
        let ws = spec();
        let findings = vec![
            finding("solo-class", "one-off failure"),
            finding("repeat-class", "first repeat"),
            finding("repeat-class", "second repeat"),
        ];
        let p = kobayashi_attack(&ws, "diff", &findings);
        let first_repeat = p.find("first repeat").expect("first repeat listed");
        let second_repeat = p.find("second repeat").expect("second repeat listed");
        let solo = p.find("one-off failure").expect("solo listed");
        assert!(
            first_repeat < solo && second_repeat < solo,
            "recurring class must be listed before the one-off"
        );
        assert!(
            first_repeat < second_repeat,
            "within a class, given order is kept"
        );
    }

    #[test]
    fn kobayashi_recurrence_tie_breaks_by_first_appearance() {
        let findings = vec![
            finding("beta", "beta finding"),
            finding("alpha", "alpha finding"),
        ];
        let ordered = order_by_recurrence(&findings);
        assert_eq!(ordered[0].weakness_class, "beta");
        assert_eq!(ordered[1].weakness_class, "alpha");
    }

    #[test]
    fn kobayashi_with_no_priors_says_so() {
        let p = kobayashi_attack(&spec(), "diff", &[]);
        assert!(p.contains("None recorded"));
    }

    #[test]
    fn comms_report_carries_objective_workstreams_and_outcomes() {
        let draft = PlanDraft {
            workstreams: vec![
                PlanDraftWorkstream {
                    slug: "part-one".into(),
                    title: "Part one".into(),
                    description: "d1".into(),
                    depends_on: vec![],
                },
                PlanDraftWorkstream {
                    slug: "part-two".into(),
                    title: "Part two".into(),
                    description: "d2".into(),
                    depends_on: vec![],
                },
            ],
        };
        let plan =
            MissionPlan::from_draft(draft, MissionId::new(), "m", "The big objective", "main")
                .unwrap();
        let p = comms_mission_report(&plan, "part-one: merged; part-two: failed");
        assert!(p.contains("The big objective"));
        assert!(p.contains("part-one: Part one"));
        assert!(p.contains("part-two: Part two"));
        assert!(p.contains("part-one: merged; part-two: failed"));
        assert!(p.contains("never invent results"));
    }
}
