//! GUI state: a pure reducer over `BridgeEvent`, fully unit-testable
//! without egui.

use bridge_core::{
    BattleReport, BridgeEvent, BudgetSnapshot, EscalationTicket, HookDecisionRecord, LogEntry,
    MergeProposal, MergeQueueEntry, MissionState, RateLimitState, TurnRecord, WorkstreamId,
    WorkstreamStatus,
};
use std::collections::HashMap;

/// Everything the panels render. Bounded buffers: logs and tactical feed
/// keep the newest `MAX_FEED` entries (drop oldest).
#[derive(Debug, Default)]
pub struct AppState {
    pub mission_state: Option<MissionState>,
    pub mission_detail: Option<String>,
    pub workstreams: HashMap<WorkstreamId, WorkstreamPanel>,
    /// Sidebar ordering: first-seen order.
    pub workstream_order: Vec<WorkstreamId>,
    pub logs: Vec<LogEntry>,
    pub tactical_feed: Vec<HookDecisionRecord>,
    pub escalations: Vec<EscalationTicket>,
    pub merge_queue: Vec<MergeQueueEntry>,
    pub pending_merges: Vec<MergeProposal>,
    pub budget: Option<BudgetSnapshot>,
    pub rate_limit: Option<RateLimitState>,
    pub compat_warning: Option<(String, String, String)>,
    pub red_alert: bool,
    pub battle_reports: Vec<BattleReport>,
}

pub const MAX_FEED: usize = 2_000;

/// Per-workstream detail pane data.
#[derive(Debug, Default)]
pub struct WorkstreamPanel {
    pub status: Option<WorkstreamStatus>,
    /// Rolling agent output (station-tagged lines), bounded MAX_FEED.
    pub output: Vec<(bridge_core::Station, String)>,
    pub tool_calls: Vec<String>,
    pub turns: Vec<TurnRecord>,
}

impl AppState {
    /// Apply one event. Total: every variant updates state; unknown data
    /// never panics. EscalationResolved removes its ticket; RateLimit
    /// Cleared clears the banner; MergeQueueUpdate replaces the queue
    /// snapshot; ConfirmMerge handling removes the proposal (done by the
    /// caller emitting fresh events, not here).
    pub fn apply(&mut self, event: BridgeEvent) {
        todo!()
    }
}
