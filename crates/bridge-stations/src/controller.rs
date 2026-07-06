//! The mission controller: the state machine that runs a mission from
//! objective to final report.
//!
//! ```text
//! StartMission
//!   -> Planning: Captain turn (PlanDraft schema) -> MissionPlan
//!      (screen_order rejects/escalates before ANY claude spawn)
//!   -> Executing: workstreams whose dependencies are merged get spawned
//!      as tokio tasks (Ops: semaphore slots come from the engine; budget
//!      checked BEFORE each turn; rate-limit gate pauses dispatch)
//!        per workstream: provision -> Helm order(s) -> Kobayashi rounds
//!        (breached -> Helm fix in the ORIGINAL worktree -> fresh round;
//!         max rounds -> Flagged, merge only via OverrideFlagged)
//!        clean -> enqueue in MergeQueue
//!   -> merge queue (serialized; see merge_queue.rs), user confirms in GUI
//!   -> after each merge: quiet-point rebases of active workstreams
//!   -> all merged -> Comms mission report -> Complete
//!
//! Pauses: BudgetExhausted / RateLimited freeze dispatch (running turns
//! finish; sessions are preserved for --resume). ExtendBudget or a
//! successful probe resumes. WindDown finishes in-flight orders then
//! reports. Shutdown persists everything and kills children gracefully.
//! ```

use crate::ports::{ComputerPort, GitPort, TacticalPort, TurnPort};
use bridge_core::{BridgeCommand, BridgeConfig, BridgeEvent, MissionPlan};
use std::path::PathBuf;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Error)]
pub enum MissionError {
    #[error("planning failed: {0}")]
    Planning(String),
    #[error("command channel closed")]
    ChannelClosed,
    #[error("{0}")]
    Fatal(String),
}

pub struct MissionController<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    _deps: std::sync::Arc<D>,
    _priv: (),
}

impl<D> MissionController<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    pub fn new(
        deps: std::sync::Arc<D>,
        config: BridgeConfig,
        helper_path: PathBuf,
        events: broadcast::Sender<BridgeEvent>,
        commands: mpsc::Receiver<BridgeCommand>,
    ) -> Self {
        todo!()
    }

    /// Resume a persisted mission instead of waiting for StartMission.
    pub fn with_resumed_plan(self, plan: MissionPlan) -> Self {
        todo!()
    }

    /// Drive the mission loop until Shutdown or completion. Emits every
    /// state change on the event bus; consumes commands. All claude turns
    /// go through deps (mockable); all git through GitPort inside
    /// spawn_blocking.
    pub async fn run(self) -> Result<(), MissionError> {
        todo!()
    }
}
