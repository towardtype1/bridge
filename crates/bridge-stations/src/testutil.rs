//! Shared mock port implementations for unit and integration tests.
//!
//! One `MockDeps` implements all four port traits with recording plus
//! per-station scripting, so every orchestration test runs with zero
//! subprocesses.

use crate::ports::{ComputerPort, GitPort, TacticalPort, TurnPort};
use bridge_compat::ClaudeInvocation;
use bridge_core::{
    BattleReport, BridgeConfig, EscalationId, Finding, HookDecisionRecord, MergeMode, MissionId,
    MissionPlan, Order, SessionId, Station, TurnRecord, UserDecision, WorkstreamId,
    WorkstreamStatus,
};
use bridge_computer::ComputerError;
use bridge_engine::{EngineError, RateLimitHit, TurnCtx, TurnOutcome};
use bridge_engine::runner::ExitClass;
use bridge_git::{GitError, MergeOutcome, RebaseOutcome, WorktreeHandle};
use bridge_tactical::{ScreenResult, WorkstreamCtx};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

/// One scripted response for a `run_turn` call.
#[derive(Debug, Clone)]
pub enum TurnResponse {
    Success {
        session: Option<String>,
        structured: Option<serde_json::Value>,
        rate_limit: Option<RateLimitHit>,
        is_error: bool,
    },
    Err(String),
    /// Never completes (aborted turn / shutdown tests).
    Hang,
}

impl TurnResponse {
    pub fn ok() -> Self {
        TurnResponse::Success {
            session: None,
            structured: None,
            rate_limit: None,
            is_error: false,
        }
    }

    pub fn structured(v: serde_json::Value) -> Self {
        TurnResponse::Success {
            session: None,
            structured: Some(v),
            rate_limit: None,
            is_error: false,
        }
    }

    pub fn rate_limited(hit: RateLimitHit) -> Self {
        TurnResponse::Success {
            session: None,
            structured: None,
            rate_limit: Some(hit),
            is_error: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordedTurn {
    pub inv: ClaudeInvocation,
    pub station: Station,
    pub kobayashi: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GitCall {
    CreateWorktree { mission: String, ws: String, base: String },
    CreateThrowaway { branch: String },
    RemoveWorktree { path: PathBuf, force: bool },
    Rebase { branch: String, target: String },
    Merge { branch: String },
    CommitsAhead { branch: String, base: String },
    WorktreeHead { path: PathBuf },
    CherryPick { path: PathBuf, branch: String, commits: Vec<String> },
    Exclude { branch: String, patterns: Vec<String> },
}

type ScreenFn = Box<dyn Fn(&Order) -> ScreenResult + Send + Sync>;

pub struct MockDeps {
    pub root: TempDir,
    // -- turn port ------------------------------------------------------
    pub turns: Mutex<Vec<RecordedTurn>>,
    pub responses: Mutex<HashMap<Station, VecDeque<TurnResponse>>>,
    pub plan_draft: Mutex<serde_json::Value>,
    pub helm_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    // -- git port --------------------------------------------------------
    pub git_calls: Mutex<Vec<GitCall>>,
    pub rebase_script: Mutex<VecDeque<RebaseOutcome>>,
    pub commits_ahead_script: Mutex<VecDeque<Vec<String>>>,
    pub changed_files: Mutex<Vec<PathBuf>>,
    pub fail_remove_worktree: Mutex<bool>,
    throwaway_counter: AtomicU32,
    // -- tactical port -----------------------------------------------------
    pub armed: Mutex<Vec<(WorkstreamId, WorkstreamCtx, String)>>,
    pub disarmed: Mutex<Vec<WorkstreamId>>,
    pub screened: Mutex<Vec<Order>>,
    pub screen_fn: Mutex<Option<ScreenFn>>,
    pub red_alert_calls: Mutex<Vec<bool>>,
    pub resolved_hook_escalations: Mutex<Vec<(EscalationId, UserDecision)>>,
    token_counter: AtomicU32,
    // -- computer port -----------------------------------------------------
    pub recorded_missions: Mutex<Vec<MissionPlan>>,
    pub recorded_statuses: Mutex<Vec<(WorkstreamId, WorkstreamStatus)>>,
    pub recorded_completes: Mutex<Vec<MissionId>>,
    pub recorded_turns: Mutex<Vec<(MissionId, TurnRecord)>>,
    pub recorded_sessions: Mutex<Vec<(WorkstreamId, Station, SessionId, PathBuf)>>,
    pub recorded_reports: Mutex<Vec<(BattleReport, Vec<PathBuf>)>>,
    pub findings: Mutex<Vec<Finding>>,
    pub findings_queries: Mutex<Vec<Vec<PathBuf>>>,
}

impl MockDeps {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            root: TempDir::new().expect("tempdir"),
            turns: Mutex::new(Vec::new()),
            responses: Mutex::new(HashMap::new()),
            plan_draft: Mutex::new(draft_json(&[("solo", &[])])),
            helm_barrier: Mutex::new(None),
            git_calls: Mutex::new(Vec::new()),
            rebase_script: Mutex::new(VecDeque::new()),
            commits_ahead_script: Mutex::new(VecDeque::new()),
            changed_files: Mutex::new(vec![PathBuf::from("src/lib.rs")]),
            fail_remove_worktree: Mutex::new(false),
            throwaway_counter: AtomicU32::new(0),
            armed: Mutex::new(Vec::new()),
            disarmed: Mutex::new(Vec::new()),
            screened: Mutex::new(Vec::new()),
            screen_fn: Mutex::new(None),
            red_alert_calls: Mutex::new(Vec::new()),
            resolved_hook_escalations: Mutex::new(Vec::new()),
            token_counter: AtomicU32::new(0),
            recorded_missions: Mutex::new(Vec::new()),
            recorded_statuses: Mutex::new(Vec::new()),
            recorded_completes: Mutex::new(Vec::new()),
            recorded_turns: Mutex::new(Vec::new()),
            recorded_sessions: Mutex::new(Vec::new()),
            recorded_reports: Mutex::new(Vec::new()),
            findings: Mutex::new(Vec::new()),
            findings_queries: Mutex::new(Vec::new()),
        })
    }

    pub fn set_plan(&self, ws: &[(&str, &[&str])]) {
        *self.plan_draft.lock().unwrap() = draft_json(ws);
    }

    pub fn push_turn(&self, station: Station, resp: TurnResponse) {
        self.responses
            .lock()
            .unwrap()
            .entry(station)
            .or_default()
            .push_back(resp);
    }

    pub fn push_rebase(&self, outcome: RebaseOutcome) {
        self.rebase_script.lock().unwrap().push_back(outcome);
    }

    pub fn push_commits_ahead(&self, commits: &[&str]) {
        self.commits_ahead_script
            .lock()
            .unwrap()
            .push_back(commits.iter().map(|c| (*c).to_string()).collect());
    }

    pub fn set_screen_fn(&self, f: impl Fn(&Order) -> ScreenResult + Send + Sync + 'static) {
        *self.screen_fn.lock().unwrap() = Some(Box::new(f));
    }

    pub fn turn_log(&self) -> Vec<RecordedTurn> {
        self.turns.lock().unwrap().clone()
    }

    pub fn git_log(&self) -> Vec<GitCall> {
        self.git_calls.lock().unwrap().clone()
    }

    pub fn statuses_for(&self, ws: WorkstreamId) -> Vec<WorkstreamStatus> {
        self.recorded_statuses
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| *id == ws)
            .map(|(_, s)| s.clone())
            .collect()
    }

    fn default_response(&self, ctx: &TurnCtx, inv: &ClaudeInvocation) -> TurnResponse {
        match ctx.station {
            Station::Captain => TurnResponse::structured(self.plan_draft.lock().unwrap().clone()),
            Station::Helm => {
                let name = inv
                    .cwd
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "ws".into());
                TurnResponse::Success {
                    session: Some(format!("sess-{name}")),
                    structured: None,
                    rate_limit: None,
                    is_error: false,
                }
            }
            Station::KobayashiMaru => {
                TurnResponse::structured(serde_json::json!({ "verdict": "clean", "findings": [] }))
            }
            _ => TurnResponse::ok(),
        }
    }
}

/// Build a Captain PlanDraft JSON value from (slug, depends_on) pairs.
pub fn draft_json(ws: &[(&str, &[&str])]) -> serde_json::Value {
    serde_json::json!({
        "workstreams": ws
            .iter()
            .map(|(slug, deps)| {
                serde_json::json!({
                    "slug": slug,
                    "title": format!("Title {slug}"),
                    "description": format!("Description {slug}"),
                    "depends_on": deps.iter().map(|d| (*d).to_string()).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>(),
    })
}

impl TurnPort for MockDeps {
    async fn run_turn(
        &self,
        inv: ClaudeInvocation,
        ctx: TurnCtx,
    ) -> Result<TurnOutcome, EngineError> {
        {
            self.turns.lock().unwrap().push(RecordedTurn {
                inv: inv.clone(),
                station: ctx.station,
                kobayashi: ctx.kobayashi,
            });
            let response = {
                let mut map = self.responses.lock().unwrap();
                map.get_mut(&ctx.station).and_then(|q| q.pop_front())
            }
            .unwrap_or_else(|| self.default_response(&ctx, &inv));
            let barrier = if ctx.station == Station::Helm {
                self.helm_barrier.lock().unwrap().clone()
            } else {
                None
            };
            if let Some(b) = barrier {
                b.wait().await;
            }
            match response {
                TurnResponse::Hang => {
                    std::future::pending::<()>().await;
                    unreachable!("pending future resolved")
                }
                TurnResponse::Err(e) => Err(EngineError::Spawn(e)),
                TurnResponse::Success {
                    session,
                    structured,
                    rate_limit,
                    is_error,
                } => {
                    let session = session.or_else(|| Some("mock-session".into()));
                    let result: bridge_compat::ResultEvent =
                        serde_json::from_value(serde_json::json!({
                            "subtype": if is_error { "error_during_execution" } else { "success" },
                            "is_error": is_error,
                            "session_id": session,
                            "num_turns": 3,
                            "duration_ms": 1200,
                            "total_cost_usd": 0.05,
                            "structured_output": structured,
                        }))
                        .expect("mock result event");
                    Ok(TurnOutcome {
                        exit: ExitClass::Success,
                        result: Some(result),
                        rate_limit,
                        structured_output: structured,
                    })
                }
            }
        }
    }
}

impl GitPort for MockDeps {
    fn create_worktree(&self, mission: &str, ws: &str, base: &str) -> Result<WorktreeHandle, GitError> {
        self.git_calls.lock().unwrap().push(GitCall::CreateWorktree {
            mission: mission.into(),
            ws: ws.into(),
            base: base.into(),
        });
        let path = self.root.path().join(mission).join(ws);
        std::fs::create_dir_all(&path).map_err(GitError::Spawn)?;
        Ok(WorktreeHandle {
            path,
            branch: format!("bridge/{mission}/{ws}"),
        })
    }

    fn create_throwaway(&self, branch: &str) -> Result<WorktreeHandle, GitError> {
        let n = self.throwaway_counter.fetch_add(1, Ordering::SeqCst);
        self.git_calls
            .lock()
            .unwrap()
            .push(GitCall::CreateThrowaway { branch: branch.into() });
        let path = self.root.path().join("throwaway").join(n.to_string());
        std::fs::create_dir_all(&path).map_err(GitError::Spawn)?;
        // Mirror the REAL create_throwaway: a DETACHED worktree whose handle
        // carries the SOURCE branch name, not a distinct ref. This is what
        // made the harvest-no-op bug (commits_ahead(branch, branch)) visible.
        Ok(WorktreeHandle {
            path,
            branch: branch.to_string(),
        })
    }

    fn worktree_head(&self, h: &WorktreeHandle) -> Result<String, GitError> {
        self.git_calls
            .lock()
            .unwrap()
            .push(GitCall::WorktreeHead { path: h.path.clone() });
        // A synthetic HEAD sha distinct from the source branch, so
        // commits_ahead(head, branch) is a non-degenerate range.
        Ok(format!("{}@throwaway-head", h.branch))
    }

    fn remove_worktree(&self, h: &WorktreeHandle, force: bool) -> Result<(), GitError> {
        self.git_calls.lock().unwrap().push(GitCall::RemoveWorktree {
            path: h.path.clone(),
            force,
        });
        if *self.fail_remove_worktree.lock().unwrap() {
            return Err(GitError::Invalid("mock remove failure".into()));
        }
        Ok(())
    }

    fn rebase_onto(&self, h: &WorktreeHandle, target: &str) -> Result<RebaseOutcome, GitError> {
        self.git_calls.lock().unwrap().push(GitCall::Rebase {
            branch: h.branch.clone(),
            target: target.into(),
        });
        Ok(self
            .rebase_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RebaseOutcome::Clean))
    }

    fn merge_into_main(&self, branch: &str, _mode: MergeMode) -> Result<MergeOutcome, GitError> {
        self.git_calls
            .lock()
            .unwrap()
            .push(GitCall::Merge { branch: branch.into() });
        Ok(MergeOutcome {
            main_head: "new-main-head".into(),
        })
    }

    fn main_branch(&self) -> Result<String, GitError> {
        Ok("main".into())
    }

    fn diff_stat_against_main(&self, branch: &str) -> Result<String, GitError> {
        Ok(format!(" {branch} | 1 file changed"))
    }

    fn changed_files_against_main(&self, _branch: &str) -> Result<Vec<PathBuf>, GitError> {
        Ok(self.changed_files.lock().unwrap().clone())
    }

    fn commits_ahead(&self, branch: &str, base: &str) -> Result<Vec<String>, GitError> {
        self.git_calls.lock().unwrap().push(GitCall::CommitsAhead {
            branch: branch.into(),
            base: base.into(),
        });
        // Mirror git: an identical ref range yields no commits. This makes
        // the old harvest bug (comparing the source branch against itself)
        // fail the harvest assertion instead of silently passing.
        if branch == base {
            return Ok(Vec::new());
        }
        Ok(self
            .commits_ahead_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default())
    }

    fn cherry_pick(&self, h: &WorktreeHandle, commits: &[String]) -> Result<Result<(), String>, GitError> {
        self.git_calls.lock().unwrap().push(GitCall::CherryPick {
            path: h.path.clone(),
            branch: h.branch.clone(),
            commits: commits.to_vec(),
        });
        Ok(Ok(()))
    }

    fn add_worktree_exclude(&self, h: &WorktreeHandle, patterns: &[&str]) -> Result<(), GitError> {
        self.git_calls.lock().unwrap().push(GitCall::Exclude {
            branch: h.branch.clone(),
            patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
        });
        Ok(())
    }

    fn rev_parse(&self, _reference: &str) -> Result<String, GitError> {
        Ok("deadbeef".into())
    }
}

impl TacticalPort for MockDeps {
    fn arm_workstream(&self, id: WorkstreamId, ctx: WorkstreamCtx) -> (String, String) {
        let n = self.token_counter.fetch_add(1, Ordering::SeqCst);
        let token = format!("tok-{n}");
        self.armed.lock().unwrap().push((id, ctx, token.clone()));
        ("http://127.0.0.1:45045".to_string(), token)
    }

    fn disarm_workstream(&self, id: WorkstreamId) {
        self.disarmed.lock().unwrap().push(id);
    }

    fn screen_order(&self, order: &Order, _station_allowed_tools: &[String]) -> ScreenResult {
        self.screened.lock().unwrap().push(order.clone());
        match &*self.screen_fn.lock().unwrap() {
            Some(f) => f(order),
            None => ScreenResult::Cleared,
        }
    }

    fn set_red_alert(&self, active: bool) {
        self.red_alert_calls.lock().unwrap().push(active);
    }

    fn resolve_hook_escalation(&self, id: EscalationId, decision: UserDecision) {
        self.resolved_hook_escalations.lock().unwrap().push((id, decision));
    }
}

impl ComputerPort for MockDeps {
    fn record_mission(&self, plan: &MissionPlan, _config: &BridgeConfig) -> Result<(), ComputerError> {
        self.recorded_missions.lock().unwrap().push(plan.clone());
        Ok(())
    }

    fn record_workstream_status(&self, ws: WorkstreamId, status: &WorkstreamStatus) -> Result<(), ComputerError> {
        self.recorded_statuses.lock().unwrap().push((ws, status.clone()));
        Ok(())
    }

    fn record_mission_complete(&self, mission: MissionId) -> Result<(), ComputerError> {
        self.recorded_completes.lock().unwrap().push(mission);
        Ok(())
    }

    fn record_turn(&self, mission: MissionId, turn: &TurnRecord) -> Result<(), ComputerError> {
        self.recorded_turns.lock().unwrap().push((mission, turn.clone()));
        Ok(())
    }

    fn record_hook_decision(&self, _mission: MissionId, _d: &HookDecisionRecord) -> Result<(), ComputerError> {
        Ok(())
    }

    fn record_session(&self, ws: WorkstreamId, station: Station, session: &SessionId, cwd: &Path) -> Result<(), ComputerError> {
        self.recorded_sessions
            .lock()
            .unwrap()
            .push((ws, station, session.clone(), cwd.to_owned()));
        Ok(())
    }

    fn session_for(&self, ws: WorkstreamId, station: Station) -> Result<Option<(SessionId, PathBuf)>, ComputerError> {
        Ok(self
            .recorded_sessions
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(w, s, _, _)| *w == ws && *s == station)
            .map(|(_, _, sid, cwd)| (sid.clone(), cwd.clone())))
    }

    fn record_battle_report(&self, report: &BattleReport, files: &[PathBuf]) -> Result<(), ComputerError> {
        self.recorded_reports
            .lock()
            .unwrap()
            .push((report.clone(), files.to_vec()));
        Ok(())
    }

    fn findings_for_files(&self, files: &[PathBuf]) -> Result<Vec<Finding>, ComputerError> {
        self.findings_queries.lock().unwrap().push(files.to_vec());
        Ok(self.findings.lock().unwrap().clone())
    }
}
