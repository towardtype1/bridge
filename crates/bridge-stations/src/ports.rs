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
    /// Root of the target repository's primary checkout; the cwd for
    /// Captain and Comms turns. Default keeps existing mocks compiling.
    fn repo_root(&self) -> PathBuf {
        PathBuf::from(".")
    }
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
        let runner = self.runner.clone();
        async move { runner.run_turn(inv, ctx).await }
    }
}

impl GitPort for LiveDeps {
    fn repo_root(&self) -> PathBuf {
        self.worktrees.repo_root().to_path_buf()
    }
    fn create_worktree(&self, mission: &str, ws: &str, base: &str) -> Result<WorktreeHandle, GitError> {
        self.worktrees.create(mission, ws, base)
    }
    fn create_throwaway(&self, branch: &str) -> Result<WorktreeHandle, GitError> {
        self.worktrees.create_throwaway(branch)
    }
    fn remove_worktree(&self, h: &WorktreeHandle, force: bool) -> Result<(), GitError> {
        self.worktrees.remove(h, force)
    }
    fn rebase_onto(&self, h: &WorktreeHandle, target: &str) -> Result<RebaseOutcome, GitError> {
        self.worktrees.rebase_onto(h, target)
    }
    fn merge_into_main(&self, branch: &str, mode: MergeMode) -> Result<MergeOutcome, GitError> {
        self.worktrees.merge_into_main(branch, mode)
    }
    fn main_branch(&self) -> Result<String, GitError> {
        self.worktrees.main_branch()
    }
    fn diff_stat_against_main(&self, branch: &str) -> Result<String, GitError> {
        self.worktrees.diff_stat_against_main(branch)
    }
    fn changed_files_against_main(&self, branch: &str) -> Result<Vec<PathBuf>, GitError> {
        self.worktrees.changed_files_against_main(branch)
    }
    fn commits_ahead(&self, branch: &str, base: &str) -> Result<Vec<String>, GitError> {
        self.worktrees.commits_ahead(branch, base)
    }
    fn cherry_pick(&self, h: &WorktreeHandle, commits: &[String]) -> Result<Result<(), String>, GitError> {
        self.worktrees.cherry_pick(h, commits)
    }
    fn add_worktree_exclude(&self, h: &WorktreeHandle, patterns: &[&str]) -> Result<(), GitError> {
        self.worktrees.add_worktree_exclude(h, patterns)
    }
    fn rev_parse(&self, reference: &str) -> Result<String, GitError> {
        self.worktrees.rev_parse(reference)
    }
}

impl TacticalPort for LiveDeps {
    fn arm_workstream(&self, id: WorkstreamId, ctx: WorkstreamCtx) -> (String, String) {
        self.policy.register_workstream(id, ctx);
        let token = self.server.issue_token(id);
        (self.server.base_url(), token)
    }
    fn disarm_workstream(&self, id: WorkstreamId) {
        self.server.revoke_token(id);
        self.policy.unregister_workstream(id);
    }
    fn screen_order(&self, order: &Order, tools: &[String]) -> ScreenResult {
        bridge_tactical::screen_order(order, tools)
    }
    fn set_red_alert(&self, active: bool) {
        self.policy.set_red_alert(active);
    }
}

impl ComputerPort for LiveDeps {
    fn record_mission(&self, plan: &MissionPlan, config: &BridgeConfig) -> Result<(), ComputerError> {
        self.computer.record_mission(plan, config)
    }
    fn record_workstream_status(&self, ws: WorkstreamId, status: &WorkstreamStatus) -> Result<(), ComputerError> {
        self.computer.record_workstream_status(ws, status)
    }
    fn record_mission_complete(&self, mission: MissionId) -> Result<(), ComputerError> {
        self.computer.record_mission_complete(mission)
    }
    fn record_turn(&self, mission: MissionId, turn: &TurnRecord) -> Result<(), ComputerError> {
        self.computer.record_turn(mission, turn)
    }
    fn record_hook_decision(&self, mission: MissionId, d: &HookDecisionRecord) -> Result<(), ComputerError> {
        self.computer.record_hook_decision(mission, d)
    }
    fn record_session(&self, ws: WorkstreamId, station: Station, session: &SessionId, cwd: &Path) -> Result<(), ComputerError> {
        self.computer.record_session(ws, station, session, cwd)
    }
    fn session_for(&self, ws: WorkstreamId, station: Station) -> Result<Option<(SessionId, PathBuf)>, ComputerError> {
        self.computer.session_for(ws, station)
    }
    fn record_battle_report(&self, report: &BattleReport, files: &[PathBuf]) -> Result<(), ComputerError> {
        self.computer.record_battle_report(report, files)
    }
    fn findings_for_files(&self, files: &[PathBuf]) -> Result<Vec<Finding>, ComputerError> {
        self.computer.findings_for_files(files)
    }
}
