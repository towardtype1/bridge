//! Kobayashi Maru: the adversarial tester.
//!
//! Every completed workstream faces it BEFORE entering the merge queue.
//! Isolation invariants (enforced here + by Tactical path policy):
//! - fresh throwaway worktree at the branch head, per round
//! - FRESH session per round (never --resume; no memory of prior attempts)
//! - writes only under tests/adversarial/
//! - it can never modify the implementation it attacks.

use crate::engineering::{self, EngineeringError, ProvisionedWorktree};
use crate::ports::{ComputerPort, GitPort, TacticalPort, TurnPort};
use crate::prompts;
use bridge_compat::{ClaudeInvocation, OutputFormat};
use bridge_core::{BattleReport, ClaudeConfig, Finding, Severity, Station, Verdict, WorkstreamSpec};
use bridge_engine::TurnCtx;
use bridge_engine::runner::ExitClass;
use bridge_git::WorktreeHandle;
use std::path::Path;

/// Max CLI turns for one adversarial round.
const KOBAYASHI_MAX_TURNS: u32 = 30;

const KOBAYASHI_ALLOWED_TOOLS: &[&str] = &["Read", "Bash", "Write", "Edit"];
const KOBAYASHI_DISALLOWED_TOOLS: &[&str] = &["WebFetch", "WebSearch", "NotebookEdit"];

#[derive(Debug, thiserror::Error)]
pub enum KobayashiError {
    #[error("tester turn failed: {0}")]
    Turn(String),
    #[error("tester produced no parseable battle report")]
    NoReport,
    #[error(transparent)]
    Git(#[from] bridge_git::GitError),
    #[error("throwaway provisioning failed: {0}")]
    Provision(#[from] EngineeringError),
}

pub struct KobayashiRunner;

impl KobayashiRunner {
    /// Run one round against `ws`'s branch:
    /// 1. provision a throwaway worktree (engineering::provision_throwaway),
    /// 2. gather changed files + prior findings from the computer,
    /// 3. one claude run: kobayashi prompt, BattleReport::json_schema
    ///    structured output, kobayashi model, FRESH session,
    /// 4. harvest committed test commits (commits_ahead of branch head)
    ///    and cherry-pick them into `impl_worktree` (the workstream's
    ///    implementation worktree, provisioned by the controller),
    /// 5. stamp + record the BattleReport (consistency-checked: an
    ///    inconsistent report becomes Breached with a synthetic finding,
    ///    fail safe),
    /// 6. decommission the throwaway (always, even on error).
    pub async fn run_round<D>(
        deps: &D,
        claude_cfg: &ClaudeConfig,
        helper_path: &Path,
        ws: &WorkstreamSpec,
        mission_slug: &str,
        impl_worktree: &WorktreeHandle,
        round: u32,
    ) -> Result<BattleReport, KobayashiError>
    where
        D: TurnPort + GitPort + TacticalPort + ComputerPort,
    {
        let branch = ws.branch_name(mission_slug);
        let provisioned =
            engineering::provision_throwaway(deps, deps, claude_cfg, helper_path, ws.id, &branch)?;
        let result =
            Self::attack(deps, claude_cfg, &provisioned, ws, impl_worktree, &branch, round).await;
        // The throwaway is always decommissioned, even when the round failed.
        engineering::decommission(deps, deps, ws.id, &provisioned, false);
        result
    }

    async fn attack<D>(
        deps: &D,
        claude_cfg: &ClaudeConfig,
        provisioned: &ProvisionedWorktree,
        ws: &WorkstreamSpec,
        impl_worktree: &WorktreeHandle,
        branch: &str,
        round: u32,
    ) -> Result<BattleReport, KobayashiError>
    where
        D: TurnPort + GitPort + TacticalPort + ComputerPort,
    {
        let changed_files = deps.changed_files_against_main(branch)?;
        // Kobayashi memory: prior findings for the touched files. Losing the
        // memory is not fatal; the round just attacks without it.
        let prior = deps.findings_for_files(&changed_files).unwrap_or_else(|e| {
            tracing::warn!("findings query failed, attacking without memory: {e}");
            Vec::new()
        });
        let diff_stat = deps.diff_stat_against_main(branch)?;

        let inv = ClaudeInvocation {
            prompt: prompts::kobayashi_attack(ws, &diff_stat, &prior),
            cwd: provisioned.handle.path.clone(),
            // Fresh session every round: no memory of prior attempts.
            resume: None,
            max_turns: KOBAYASHI_MAX_TURNS,
            allowed_tools: KOBAYASHI_ALLOWED_TOOLS.iter().map(|s| (*s).to_string()).collect(),
            disallowed_tools: KOBAYASHI_DISALLOWED_TOOLS.iter().map(|s| (*s).to_string()).collect(),
            model: Some(claude_cfg.kobayashi_model.clone()),
            json_schema: Some(BattleReport::json_schema()),
            mcp_config: None,
            append_system_prompt: Some(
                "You are the Kobayashi Maru: an adversarial tester. Break the change under \
                 test; write failing tests only under tests/adversarial/."
                    .into(),
            ),
            output_format: OutputFormat::Json,
            setting_sources: vec!["project".into(), "local".into()],
        };
        let ctx = TurnCtx {
            workstream: ws.id,
            station: Station::KobayashiMaru,
            kobayashi: true,
            pid_register: None,
        };

        let outcome = deps
            .run_turn(inv, ctx)
            .await
            .map_err(|e| KobayashiError::Turn(e.to_string()))?;
        if outcome.exit != ExitClass::Success {
            return Err(KobayashiError::Turn(format!(
                "claude exited abnormally: {:?}",
                outcome.exit
            )));
        }
        if let Some(result) = &outcome.result
            && result.is_error
        {
            return Err(KobayashiError::Turn(format!(
                "result reported error subtype {}",
                result.subtype
            )));
        }

        let structured = outcome
            .structured_output
            .clone()
            .or_else(|| outcome.result.as_ref().and_then(|r| r.structured_output.clone()))
            .ok_or(KobayashiError::NoReport)?;
        let mut report = BattleReport::from_model_output(&structured, ws.id, round)
            .map_err(|_| KobayashiError::NoReport)?;

        // Harvest committed adversarial tests: commits the tester made in
        // the throwaway, cherry-picked into the implementation worktree.
        let commits = deps.commits_ahead(&provisioned.handle.branch, branch)?;
        if !commits.is_empty() {
            match deps.cherry_pick(impl_worktree, &commits) {
                Ok(Ok(())) => {}
                Ok(Err(conflict)) => {
                    tracing::warn!("cherry-pick of adversarial tests conflicted at {conflict}");
                }
                Err(e) => tracing::warn!("cherry-pick of adversarial tests failed: {e}"),
            }
        }

        // Fail safe: an inconsistent report is treated as a breach.
        if !report.is_consistent() {
            tracing::warn!(
                "inconsistent battle report ({:?} with {} findings); converting to breached",
                report.verdict,
                report.findings.len()
            );
            report.verdict = Verdict::Breached;
            report.findings.push(Finding {
                severity: Severity::High,
                title: "Inconsistent battle report".into(),
                description: "The tester's verdict did not match its findings; failing safe to \
                              breached."
                    .into(),
                reproduction_command: String::new(),
                failing_test_path: None,
                weakness_class: "inconsistent-report".into(),
            });
        }

        if let Err(e) = deps.record_battle_report(&report, &changed_files) {
            tracing::warn!("failed to record battle report: {e}");
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{GitCall, MockDeps, TurnResponse};
    use bridge_core::WorkstreamId;
    use std::path::PathBuf;

    fn spec(slug: &str) -> WorkstreamSpec {
        WorkstreamSpec {
            id: WorkstreamId::new(),
            slug: slug.into(),
            title: format!("Title {slug}"),
            description: format!("Description {slug}"),
            base_ref: "main".into(),
        }
    }

    fn cfg() -> ClaudeConfig {
        ClaudeConfig {
            kobayashi_model: "the-strongest-model".into(),
            ..ClaudeConfig::default()
        }
    }

    fn helper() -> PathBuf {
        PathBuf::from("/opt/bridge/bridge-hook-helper")
    }

    /// The implementation worktree handle the controller would pass, laid
    /// out where the mock git expects it.
    fn impl_handle(deps: &MockDeps, mission: &str, ws: &WorkstreamSpec) -> WorktreeHandle {
        WorktreeHandle {
            path: deps.root.path().join(mission).join(&ws.slug),
            branch: ws.branch_name(mission),
        }
    }

    fn throwaway_removed(deps: &MockDeps) -> bool {
        deps.git_log().iter().any(|c| {
            matches!(c, GitCall::RemoveWorktree { path, force: true } if path.starts_with(deps.root.path().join("throwaway")))
        })
    }

    #[tokio::test]
    async fn clean_round_records_report_and_harvests_tests() {
        let deps = MockDeps::new();
        deps.push_commits_ahead(&["c1", "c2"]);
        let ws = spec("ws-a");

        let report = KobayashiRunner::run_round(
            &*deps,
            &cfg(),
            &helper(),
            &ws,
            "mission-x",
            &impl_handle(&deps, "mission-x", &ws),
            1,
        )
        .await
        .expect("round succeeds");

        assert_eq!(report.verdict, Verdict::Clean);
        assert_eq!(report.workstream, ws.id);
        assert_eq!(report.round, 1);
        assert!(report.findings.is_empty());

        // Exactly one turn: fresh session, kobayashi flag, kobayashi model,
        // BattleReport schema, run in the throwaway worktree.
        let turns = deps.turn_log();
        assert_eq!(turns.len(), 1);
        let t = &turns[0];
        assert_eq!(t.station, Station::KobayashiMaru);
        assert!(t.kobayashi, "TurnCtx must carry the kobayashi flag");
        assert_eq!(t.inv.resume, None, "kobayashi never resumes a session");
        assert_eq!(t.inv.model.as_deref(), Some("the-strongest-model"));
        assert_eq!(t.inv.json_schema, Some(BattleReport::json_schema()));
        assert!(t.inv.cwd.starts_with(deps.root.path().join("throwaway")));
        assert!(t.inv.prompt.contains("tests/adversarial/"));

        // Committed tests harvested into the implementation worktree.
        let git = deps.git_log();
        assert!(git.contains(&GitCall::CommitsAhead {
            branch: "bridge/mission-x/ws-a@throwaway0".into(),
            base: "bridge/mission-x/ws-a".into(),
        }));
        assert!(git.iter().any(|c| matches!(
            c,
            GitCall::CherryPick { path, branch, commits }
                if branch == "bridge/mission-x/ws-a"
                    && commits == &vec!["c1".to_string(), "c2".to_string()]
                    && path == &deps.root.path().join("mission-x").join("ws-a")
        )));

        // Report recorded against the changed files.
        let recorded = deps.recorded_reports.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, report);
        assert_eq!(recorded[0].1, vec![PathBuf::from("src/lib.rs")]);
        drop(recorded);

        // Throwaway decommissioned: disarmed and force-removed.
        assert_eq!(deps.disarmed.lock().unwrap().as_slice(), &[ws.id]);
        assert!(throwaway_removed(&deps));
    }

    #[tokio::test]
    async fn no_commits_means_no_cherry_pick() {
        let deps = MockDeps::new();
        let ws = spec("ws-a");
        KobayashiRunner::run_round(&*deps, &cfg(), &helper(), &ws, "m", &impl_handle(&deps, "m", &ws), 1)
            .await
            .unwrap();
        assert!(
            !deps.git_log().iter().any(|c| matches!(c, GitCall::CherryPick { .. })),
            "no commits ahead -> no cherry-pick"
        );
    }

    #[tokio::test]
    async fn prior_findings_feed_the_attack_prompt() {
        let deps = MockDeps::new();
        deps.findings.lock().unwrap().push(Finding {
            severity: Severity::Critical,
            title: "old wound".into(),
            description: "it broke here before".into(),
            reproduction_command: "cargo test old_wound".into(),
            failing_test_path: None,
            weakness_class: "path-traversal".into(),
        });
        let ws = spec("ws-a");
        KobayashiRunner::run_round(&*deps, &cfg(), &helper(), &ws, "m", &impl_handle(&deps, "m", &ws), 2)
            .await
            .unwrap();
        let turns = deps.turn_log();
        assert!(turns[0].inv.prompt.contains("old wound"));
        assert!(turns[0].inv.prompt.contains("path-traversal"));
        // The memory was queried with the workstream's changed files.
        assert_eq!(
            deps.findings_queries.lock().unwrap()[0],
            vec![PathBuf::from("src/lib.rs")]
        );
    }

    #[tokio::test]
    async fn inconsistent_report_becomes_breached_with_synthetic_finding() {
        let deps = MockDeps::new();
        deps.push_turn(
            Station::KobayashiMaru,
            TurnResponse::structured(serde_json::json!({ "verdict": "breached", "findings": [] })),
        );
        let ws = spec("ws-a");
        let report =
            KobayashiRunner::run_round(&*deps, &cfg(), &helper(), &ws, "m", &impl_handle(&deps, "m", &ws), 1)
                .await
                .unwrap();
        assert_eq!(report.verdict, Verdict::Breached);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].weakness_class, "inconsistent-report");
        assert!(report.is_consistent(), "converted report must be consistent");
        // The recorded copy is the converted one.
        assert_eq!(deps.recorded_reports.lock().unwrap()[0].0, report);
    }

    #[tokio::test]
    async fn clean_verdict_with_findings_is_also_converted() {
        let deps = MockDeps::new();
        deps.push_turn(
            Station::KobayashiMaru,
            TurnResponse::structured(serde_json::json!({
                "verdict": "clean",
                "findings": [{
                    "severity": "low",
                    "title": "t",
                    "description": "d",
                    "reproduction_command": "true",
                    "weakness_class": "misc"
                }]
            })),
        );
        let ws = spec("ws-a");
        let report =
            KobayashiRunner::run_round(&*deps, &cfg(), &helper(), &ws, "m", &impl_handle(&deps, "m", &ws), 1)
                .await
                .unwrap();
        assert_eq!(report.verdict, Verdict::Breached);
        assert_eq!(report.findings.len(), 2, "original finding kept + synthetic added");
    }

    #[tokio::test]
    async fn turn_failure_is_turn_error_and_still_decommissions() {
        let deps = MockDeps::new();
        deps.push_turn(Station::KobayashiMaru, TurnResponse::Err("boom".into()));
        let ws = spec("ws-a");
        let err =
            KobayashiRunner::run_round(&*deps, &cfg(), &helper(), &ws, "m", &impl_handle(&deps, "m", &ws), 1)
                .await
                .unwrap_err();
        assert!(matches!(err, KobayashiError::Turn(_)), "got {err:?}");
        assert_eq!(deps.disarmed.lock().unwrap().as_slice(), &[ws.id]);
        assert!(throwaway_removed(&deps), "throwaway must be decommissioned on error");
        assert!(deps.recorded_reports.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_structured_output_is_no_report_and_still_decommissions() {
        let deps = MockDeps::new();
        deps.push_turn(Station::KobayashiMaru, TurnResponse::ok());
        let ws = spec("ws-a");
        let err =
            KobayashiRunner::run_round(&*deps, &cfg(), &helper(), &ws, "m", &impl_handle(&deps, "m", &ws), 1)
                .await
                .unwrap_err();
        assert!(matches!(err, KobayashiError::NoReport), "got {err:?}");
        assert!(throwaway_removed(&deps));
    }
}
