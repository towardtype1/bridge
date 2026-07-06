//! Port traits: the seams between orchestration logic and the live
//! engine/git/tactical/computer crates, so the mission state machine is
//! unit-testable with mocks and no subprocesses.
//!
//! Live adapters (`LiveDeps`) delegate 1:1 to the real crates. Git methods
//! are blocking; the controller calls them via `tokio::task::spawn_blocking`.

use bridge_compat::ClaudeInvocation;
use bridge_core::{
    BattleReport, BridgeConfig, Finding, HookDecisionRecord, MergeMode, MissionId, MissionPlan,
    Order, SessionId, Station, TurnRecord, WorkstreamId, WorkstreamStatus,
};
use bridge_engine::{ClaudeRunner, EngineError, TurnCtx, TurnOutcome};
use bridge_git::{GitError, MergeOutcome, RebaseOutcome, WorktreeHandle, WorktreeManager};
use bridge_tactical::{ControlServerHandle, PolicyEngine, ScreenResult, WorkstreamCtx};
use bridge_computer::{ComputerError, ShipsComputer};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait TurnPort: Send + Sync + 'static {
    fn run_turn(
        &self,
        inv: ClaudeInvocation,
        ctx: TurnCtx,
    ) -> impl Future<Output = Result<TurnOutcome, EngineError>> + Send;
}

pub trait GitPort: Send + Sync + 'static {
    fn create_worktree(&self, mission: &str, ws: &str, base: &str) -> Result<WorktreeHandle, GitError>;
    fn create_throwaway(&self, branch: &str) -> Result<WorktreeHandle, GitError>;
    fn remove_worktree(&self, h: &WorktreeHandle, force: bool) -> Result<(), GitError>;
    fn rebase_onto(&self, h: &WorktreeHandle, target: &str) -> Result<RebaseOutcome, GitError>;
    fn merge_into_main(&self, branch: &str, mode: MergeMode) -> Result<MergeOutcome, GitError>;
    fn main_branch(&self) -> Result<String, GitError>;
    fn diff_stat_against_main(&self, branch: &str) -> Result<String, GitError>;
    fn changed_files_against_main(&self, branch: &str) -> Result<Vec<PathBuf>, GitError>;
    fn commits_ahead(&self, branch: &str, base: &str) -> Result<Vec<String>, GitError>;
    fn cherry_pick(&self, h: &WorktreeHandle, commits: &[String]) -> Result<Result<(), String>, GitError>;
    fn add_worktree_exclude(&self, h: &WorktreeHandle, patterns: &[&str]) -> Result<(), GitError>;
    fn rev_parse(&self, reference: &str) -> Result<String, GitError>;
}

pub trait TacticalPort: Send + Sync + 'static {
    /// Register the workstream with the policy engine and issue a hook
    /// token. Returns (server_base_url, token) for settings rendering.
    fn arm_workstream(&self, id: WorkstreamId, ctx: WorkstreamCtx) -> (String, String);
    fn disarm_workstream(&self, id: WorkstreamId);
    fn screen_order(&self, order: &Order, station_allowed_tools: &[String]) -> ScreenResult;
    fn set_red_alert(&self, active: bool);
}

pub trait ComputerPort: Send + Sync + 'static {
    fn record_mission(&self, plan: &MissionPlan, config: &BridgeConfig) -> Result<(), ComputerError>;
    fn record_workstream_status(&self, ws: WorkstreamId, status: &WorkstreamStatus) -> Result<(), ComputerError>;
    fn record_mission_complete(&self, mission: MissionId) -> Result<(), ComputerError>;
    fn record_turn(&self, mission: MissionId, turn: &TurnRecord) -> Result<(), ComputerError>;
    fn record_hook_decision(&self, mission: MissionId, d: &HookDecisionRecord) -> Result<(), ComputerError>;
    fn record_session(&self, ws: WorkstreamId, station: Station, session: &SessionId, cwd: &Path) -> Result<(), ComputerError>;
    fn session_for(&self, ws: WorkstreamId, station: Station) -> Result<Option<(SessionId, PathBuf)>, ComputerError>;
    fn record_battle_report(&self, report: &BattleReport, files: &[PathBuf]) -> Result<(), ComputerError>;
    fn findings_for_files(&self, files: &[PathBuf]) -> Result<Vec<Finding>, ComputerError>;
}

/// Live wiring of the four ports to the real crates.
pub struct LiveDeps {
    pub runner: Arc<ClaudeRunner>,
    pub worktrees: Arc<WorktreeManager>,
    pub policy: Arc<PolicyEngine>,
    pub server: Arc<ControlServerHandle>,
    pub computer: Arc<ShipsComputer>,
}

impl TurnPort for LiveDeps {
    fn run_turn(
        &self,
        inv: ClaudeInvocation,
        ctx: TurnCtx,
    ) -> impl Future<Output = Result<TurnOutcome, EngineError>> + Send {
        async move { todo!() }
    }
}

impl GitPort for LiveDeps {
    fn create_worktree(&self, _mission: &str, _ws: &str, _base: &str) -> Result<WorktreeHandle, GitError> { todo!() }
    fn create_throwaway(&self, _branch: &str) -> Result<WorktreeHandle, GitError> { todo!() }
    fn remove_worktree(&self, _h: &WorktreeHandle, _force: bool) -> Result<(), GitError> { todo!() }
    fn rebase_onto(&self, _h: &WorktreeHandle, _target: &str) -> Result<RebaseOutcome, GitError> { todo!() }
    fn merge_into_main(&self, _branch: &str, _mode: MergeMode) -> Result<MergeOutcome, GitError> { todo!() }
    fn main_branch(&self) -> Result<String, GitError> { todo!() }
    fn diff_stat_against_main(&self, _branch: &str) -> Result<String, GitError> { todo!() }
    fn changed_files_against_main(&self, _branch: &str) -> Result<Vec<PathBuf>, GitError> { todo!() }
    fn commits_ahead(&self, _branch: &str, _base: &str) -> Result<Vec<String>, GitError> { todo!() }
    fn cherry_pick(&self, _h: &WorktreeHandle, _commits: &[String]) -> Result<Result<(), String>, GitError> { todo!() }
    fn add_worktree_exclude(&self, _h: &WorktreeHandle, _patterns: &[&str]) -> Result<(), GitError> { todo!() }
    fn rev_parse(&self, _reference: &str) -> Result<String, GitError> { todo!() }
}

impl TacticalPort for LiveDeps {
    fn arm_workstream(&self, _id: WorkstreamId, _ctx: WorkstreamCtx) -> (String, String) { todo!() }
    fn disarm_workstream(&self, _id: WorkstreamId) { todo!() }
    fn screen_order(&self, _order: &Order, _tools: &[String]) -> ScreenResult { todo!() }
    fn set_red_alert(&self, _active: bool) { todo!() }
}

impl ComputerPort for LiveDeps {
    fn record_mission(&self, _plan: &MissionPlan, _config: &BridgeConfig) -> Result<(), ComputerError> { todo!() }
    fn record_workstream_status(&self, _ws: WorkstreamId, _status: &WorkstreamStatus) -> Result<(), ComputerError> { todo!() }
    fn record_mission_complete(&self, _mission: MissionId) -> Result<(), ComputerError> { todo!() }
    fn record_turn(&self, _mission: MissionId, _turn: &TurnRecord) -> Result<(), ComputerError> { todo!() }
    fn record_hook_decision(&self, _mission: MissionId, _d: &HookDecisionRecord) -> Result<(), ComputerError> { todo!() }
    fn record_session(&self, _ws: WorkstreamId, _station: Station, _session: &SessionId, _cwd: &Path) -> Result<(), ComputerError> { todo!() }
    fn session_for(&self, _ws: WorkstreamId, _station: Station) -> Result<Option<(SessionId, PathBuf)>, ComputerError> { todo!() }
    fn record_battle_report(&self, _report: &BattleReport, _files: &[PathBuf]) -> Result<(), ComputerError> { todo!() }
    fn findings_for_files(&self, _files: &[PathBuf]) -> Result<Vec<Finding>, ComputerError> { todo!() }
}
