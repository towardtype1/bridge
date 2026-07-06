//! The Ship's Computer: SQLite persistence for everything worth remembering.
//!
//! One database per target repository (path chosen by the app). WAL mode.
//! All methods take `&self`; internal Mutex<Connection> keeps this Send +
//! Sync so stations can share an Arc<ShipsComputer>.

use bridge_core::{
    BattleReport, BridgeConfig, BudgetSnapshot, Finding, HookDecisionRecord, MissionId,
    MissionPlan, SessionId, Station, TurnRecord, WorkstreamId, WorkstreamStatus,
};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComputerError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// A mission as persisted for resume-after-restart.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedMission {
    pub plan: MissionPlan,
    pub statuses: Vec<(WorkstreamId, WorkstreamStatus)>,
    /// Config snapshot taken when the mission started.
    pub config: BridgeConfig,
}

pub struct ShipsComputer {
    // Mutex<rusqlite::Connection> internally.
    _priv: (),
}

impl ShipsComputer {
    /// Open (creating if needed) and migrate. Migrations are idempotent and
    /// versioned via `PRAGMA user_version`.
    pub fn open(db_path: &Path) -> Result<Self, ComputerError> {
        todo!()
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> Result<Self, ComputerError> {
        todo!()
    }

    // -- mission lifecycle ---------------------------------------------------

    /// Insert or replace the mission plan + config snapshot.
    pub fn record_mission(&self, plan: &MissionPlan, config: &BridgeConfig) -> Result<(), ComputerError> {
        todo!()
    }

    pub fn record_workstream_status(&self, ws: WorkstreamId, status: &WorkstreamStatus) -> Result<(), ComputerError> {
        todo!()
    }

    pub fn record_mission_complete(&self, mission: MissionId) -> Result<(), ComputerError> {
        todo!()
    }

    /// The most recent incomplete mission, if any (offer resume at startup).
    pub fn persisted_mission(&self) -> Result<Option<PersistedMission>, ComputerError> {
        todo!()
    }

    // -- turns, usage, decisions ----------------------------------------------

    pub fn record_turn(&self, mission: MissionId, turn: &TurnRecord) -> Result<(), ComputerError> {
        todo!()
    }

    pub fn record_hook_decision(&self, mission: MissionId, d: &HookDecisionRecord) -> Result<(), ComputerError> {
        todo!()
    }

    /// Live budget usage derived from recorded turns + mission start time.
    pub fn usage_snapshot(&self, mission: MissionId, cfg: &BridgeConfig) -> Result<BudgetSnapshot, ComputerError> {
        todo!()
    }

    /// Turn counts per station for the GUI header.
    pub fn turns_by_station(&self, mission: MissionId) -> Result<Vec<(Station, u32)>, ComputerError> {
        todo!()
    }

    // -- sessions --------------------------------------------------------------

    /// Persist the claude session for (workstream, station) with the cwd it
    /// must be resumed from. Upsert.
    pub fn record_session(&self, ws: WorkstreamId, station: Station, session: &SessionId, cwd: &Path) -> Result<(), ComputerError> {
        todo!()
    }

    pub fn session_for(&self, ws: WorkstreamId, station: Station) -> Result<Option<(SessionId, PathBuf)>, ComputerError> {
        todo!()
    }

    // -- battle reports ----------------------------------------------------------

    /// Store a report and index findings by touched file paths extracted
    /// from `files` (caller supplies the diff file list of the workstream).
    pub fn record_battle_report(&self, report: &BattleReport, files: &[PathBuf]) -> Result<(), ComputerError> {
        todo!()
    }

    /// Past findings whose indexed files intersect `files`, newest first.
    /// This is the Kobayashi Maru memory: recurring weakness classes for
    /// in-scope files get attacked first.
    pub fn findings_for_files(&self, files: &[PathBuf]) -> Result<Vec<Finding>, ComputerError> {
        todo!()
    }

    // -- child process registry (orphan cleanup) ---------------------------------

    pub fn register_child_pid(&self, pid: u32, workstream: WorkstreamId) -> Result<(), ComputerError> {
        todo!()
    }

    pub fn clear_child_pid(&self, pid: u32) -> Result<(), ComputerError> {
        todo!()
    }

    /// Pids recorded as live from a previous run; the app probes and kills
    /// survivors at startup, then clears them.
    pub fn recorded_child_pids(&self) -> Result<Vec<u32>, ComputerError> {
        todo!()
    }
}
