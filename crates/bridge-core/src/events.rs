//! The event bus vocabulary: everything the engine tells the GUI.
//!
//! Every variant is cheap to clone; the bus is a tokio broadcast channel
//! owned by the app crate. This module stays runtime-agnostic.

use crate::ids::{EscalationId, MissionId, OrderId, SessionId, Station, WorkstreamId};
use crate::plan::{PlanDiff, PlanDraft};
use crate::report::BattleReport;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Any observable happening in the harness, streamed live to the GUI and
/// recorded by the Ship's Computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BridgeEvent {
    Log(LogEntry),
    MissionStatus(MissionStatusUpdate),
    WorkstreamStatus {
        id: WorkstreamId,
        status: WorkstreamStatus,
    },
    /// A chunk of assistant text produced by a station agent.
    AgentOutput {
        workstream: WorkstreamId,
        station: Station,
        text: String,
    },
    /// A tool invocation observed in the stream (pre-adjudication view).
    ToolCall {
        workstream: WorkstreamId,
        station: Station,
        tool_name: String,
        summary: String,
    },
    TurnCompleted(TurnRecord),
    HookDecision(HookDecisionRecord),
    EscalationRequested(EscalationTicket),
    EscalationResolved {
        id: EscalationId,
        decision: UserDecision,
    },
    RateLimit(RateLimitState),
    BudgetUpdate(BudgetSnapshot),
    MergeQueueUpdate(Vec<MergeQueueEntry>),
    MergeConfirmationRequested(MergeProposal),
    BattleReportFiled(BattleReport),
    /// CLI version outside the tested range; GUI shows a prominent warning.
    CompatWarning {
        detected: String,
        tested_min: String,
        tested_max: String,
    },
    /// A workstream's worktree was created on disk; carries its path so the
    /// GUI can offer "Open in editor".
    WorkstreamProvisioned {
        id: WorkstreamId,
        worktree_path: PathBuf,
    },
    /// User message accepted into the Captain conference (canonical echo).
    UserSaid {
        mission: MissionId,
        text: String,
    },
    /// One completed Captain conference turn's message.
    CaptainSays {
        mission: MissionId,
        text: String,
    },
    /// A versioned plan proposal; `diff` is present for amendments.
    PlanProposed {
        mission: MissionId,
        revision: u64,
        plan: PlanDraft,
        diff: Option<PlanDiff>,
    },
    /// Stale approval, validation failure, or conference cap reached.
    ProposalRejected {
        mission: MissionId,
        revision: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub station: Option<Station>,
    pub workstream: Option<WorkstreamId>,
    pub message: String,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            station: None,
            workstream: None,
            message: message.into(),
        }
    }

    pub fn station(mut self, s: Station) -> Self {
        self.station = Some(s);
        self
    }

    pub fn workstream(mut self, w: WorkstreamId) -> Self {
        self.workstream = Some(w);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionStatusUpdate {
    pub mission: MissionId,
    pub state: MissionState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MissionState {
    Planning,
    Executing,
    Paused { reason: PauseReason },
    WindingDown,
    Complete,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PauseReason {
    RateLimited {
        retry_at: Option<DateTime<Utc>>,
    },
    /// Which budget ran out, e.g. "max_total_turns".
    BudgetExhausted {
        which: String,
    },
    UserRequested,
}

/// Lifecycle of a single workstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkstreamStatus {
    Pending,
    Working,
    /// Kobayashi Maru round in progress.
    UnderTest {
        round: u32,
    },
    /// Kobayashi Maru found failures; a fix order is running or queued.
    Breached {
        round: u32,
    },
    ReadyToMerge,
    Rebasing,
    /// Rebase conflicts being fixed by a Helm order.
    ConflictFix,
    InMergeQueue,
    Merged,
    Failed {
        reason: String,
    },
    /// Still breached after max Kobayashi rounds; merge needs user override.
    Flagged,
    /// Removed from the plan by an approved amendment before it started.
    Cancelled,
}

/// Outcome of one claude invocation, as recorded in the Ship's Computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub order: OrderId,
    pub workstream: WorkstreamId,
    pub station: Station,
    pub session: Option<SessionId>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub num_turns: u32,
    pub is_error: bool,
    /// `result.subtype` verbatim ("success", "error_max_turns", ...).
    pub subtype: String,
    pub total_cost_usd: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionKind {
    Allow,
    Deny,
    Escalate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionSource {
    PrimeDirective,
    ConfigRule,
    DeepScan,
    User,
    /// No rule fired; the call passed through to the static permission gate.
    Passthrough,
}

/// One adjudicated hook call: what Tactical decided and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookDecisionRecord {
    pub timestamp: DateTime<Utc>,
    pub workstream: WorkstreamId,
    pub hook_event: String,
    pub tool_name: Option<String>,
    pub decision: DecisionKind,
    pub reason: Option<String>,
    pub rule: String,
    pub source: DecisionSource,
    pub latency_ms: u64,
}

/// A question for the user, blocking one tool call until answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EscalationTicket {
    pub id: EscalationId,
    pub workstream: WorkstreamId,
    pub question: String,
    pub tool_name: Option<String>,
    /// Compact rendering of tool_input for display.
    pub tool_input_summary: String,
    pub requested_at: DateTime<Utc>,
    /// After this instant the broker fails closed to deny.
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UserDecision {
    Approve,
    Deny { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RateLimitState {
    Hit { retry_at: Option<DateTime<Utc>> },
    Cleared,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkstreamTurns {
    pub workstream: WorkstreamId,
    pub turns: u32,
}

/// Live usage against the mission budgets, shown in the GUI header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    pub mission: MissionId,
    pub total_turns: u32,
    pub max_total_turns: u32,
    pub per_workstream: Vec<WorkstreamTurns>,
    pub wall_clock_secs: u64,
    pub max_wall_clock_secs: Option<u64>,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeQueueState {
    AwaitingRebase,
    Rebasing,
    ConflictFix,
    ChecksRunning,
    AwaitingConfirmation,
    Merging,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeQueueEntry {
    pub workstream: WorkstreamId,
    pub branch: String,
    pub position: u32,
    pub state: MergeQueueState,
}

/// A merge awaiting explicit user confirmation in the GUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeProposal {
    pub workstream: WorkstreamId,
    pub branch: String,
    pub target: String,
    pub summary: String,
    /// `git diff --stat` style overview for display.
    pub diff_stat: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serde_round_trip() {
        let e = BridgeEvent::HookDecision(HookDecisionRecord {
            timestamp: Utc::now(),
            workstream: WorkstreamId::new(),
            hook_event: "PreToolUse".into(),
            tool_name: Some("Bash".into()),
            decision: DecisionKind::Deny,
            reason: Some("rm -rf outside worktree".into()),
            rule: "prime_directive.worktree_escape".into(),
            source: DecisionSource::PrimeDirective,
            latency_ms: 3,
        });
        let json = serde_json::to_string(&e).unwrap();
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn log_entry_builder_attaches_context() {
        let ws = WorkstreamId::new();
        let entry = LogEntry::new(LogLevel::Info, "engaged")
            .station(Station::Helm)
            .workstream(ws);
        assert_eq!(entry.station, Some(Station::Helm));
        assert_eq!(entry.workstream, Some(ws));
    }
}
