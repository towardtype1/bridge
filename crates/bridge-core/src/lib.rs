//! Shared types for the Bridge harness.
//!
//! This crate is the interface contract between every other Bridge crate.
//! It is deliberately dependency-light (no tokio, no I/O beyond config
//! loading) so that the hook helper binary can depend on it without
//! paying a startup cost.

pub mod commands;
pub mod config;
pub mod events;
pub mod ids;
pub mod orders;
pub mod plan;
pub mod report;
pub mod wire;

pub use commands::{BridgeCommand, BudgetExtension};
pub use config::{
    BridgeConfig, BudgetConfig, ClaudeConfig, ConfigError, DeckPalette, LinearConfig,
    MergeConfig, MergeMode, TacticalConfig, UiConfig, WorktreeConfig,
};
pub use events::{
    BridgeEvent, BudgetSnapshot, DecisionKind, DecisionSource, EscalationTicket,
    HookDecisionRecord, LogEntry, LogLevel, MergeProposal, MergeQueueEntry, MergeQueueState,
    MissionState, MissionStatusUpdate, PauseReason, RateLimitState, TurnRecord, UserDecision,
    WorkstreamStatus, WorkstreamTurns,
};
pub use ids::{EscalationId, MissionId, OrderId, SessionId, Station, WorkstreamId};
pub use orders::Order;
pub use plan::{MissionPlan, PlanDraft, PlanDraftWorkstream, PlanError, WorkstreamSpec};
pub use report::{BattleReport, Finding, Severity, Verdict};
