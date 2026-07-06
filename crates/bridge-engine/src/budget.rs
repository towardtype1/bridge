//! Budget accounting and enforcement (owned by Ops, mechanics live here).

use bridge_core::{BudgetConfig, BudgetExtension, BudgetSnapshot, MissionId, WorkstreamId};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetStatus {
    Ok,
    /// Which ceiling was hit, e.g. "max_total_turns",
    /// "max_turns_per_workstream:<slug>", "max_wall_clock_secs".
    Exhausted { which: String },
}

/// Thread-safe ledger. Time is injected so tests are deterministic.
pub struct BudgetLedger {
    _priv: (),
}

impl BudgetLedger {
    pub fn new(mission: MissionId, config: BudgetConfig, started_at: DateTime<Utc>) -> Self {
        todo!()
    }

    /// Record a completed turn (any station) against a workstream.
    pub fn record_turn(&self, ws: WorkstreamId, cost_usd: Option<f64>) {
        todo!()
    }

    /// Kobayashi rounds are budgeted separately per workstream.
    pub fn record_kobayashi_round(&self, ws: WorkstreamId) -> u32 {
        todo!()
    }

    pub fn kobayashi_rounds(&self, ws: WorkstreamId) -> u32 {
        todo!()
    }

    /// Check BEFORE dispatching a turn; `now` injected for testability.
    pub fn check(&self, ws: WorkstreamId, now: DateTime<Utc>) -> BudgetStatus {
        todo!()
    }

    pub fn extend(&self, ext: BudgetExtension) {
        todo!()
    }

    pub fn snapshot(&self, now: DateTime<Utc>) -> BudgetSnapshot {
        todo!()
    }
}
