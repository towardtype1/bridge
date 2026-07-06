//! Commands flowing from the GUI to the mission controller.

use crate::events::UserDecision;
use crate::ids::{EscalationId, WorkstreamId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BridgeCommand {
    /// Start a mission from a user objective. The Captain plans it.
    StartMission { objective: String },
    /// Answer an escalation ticket.
    ResolveEscalation {
        id: EscalationId,
        decision: UserDecision,
    },
    /// Confirm or reject a proposed merge to main.
    ConfirmMerge {
        workstream: WorkstreamId,
        approved: bool,
    },
    /// Raise budget ceilings on a paused mission.
    ExtendBudget(BudgetExtension),
    /// Finish in-flight orders, produce a status summary, stop.
    WindDown,
    /// Let a workstream that stayed breached past max Kobayashi rounds
    /// enter the merge queue anyway. Explicit user override only.
    OverrideFlagged { workstream: WorkstreamId },
    /// Graceful shutdown: terminate children, persist state, clean up.
    Shutdown,
    /// Re-emit all currently-actionable one-shot signals (open escalations
    /// and pending merge proposals). A consumer sends this after it detects
    /// it lagged the event bus, so a dropped `EscalationRequested` /
    /// `MergeConfirmationRequested` cannot silently strand the mission.
    ResyncActionable,
}

/// Additional headroom granted to a paused mission. Zero fields leave the
/// corresponding ceiling unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BudgetExtension {
    pub extra_total_turns: u32,
    pub extra_turns_per_workstream: u32,
    pub extra_wall_clock_secs: u64,
}
