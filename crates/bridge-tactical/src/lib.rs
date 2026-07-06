//! Tactical: guardrails for every tool call and every order.
//!
//! Decision order (spec, never reordered):
//! 1. Prime Directives - hard denies, never overridable, hardcoded.
//! 2. Rule-based policy from config: destructive command patterns,
//!    protected path patterns, worktree path allowlist, egress rules.
//! 3. Red Alert - when active, EVERY tool call escalates to the GUI.
//! 4. Optional deep scan - suspicious-but-not-denied calls may get a
//!    one-shot cheap claude review before deciding (owned by the server
//!    layer; the policy engine only marks `suspicious`).
//!
//! The control server fails CLOSED end to end: unreachable server or an
//! expired escalation resolves to deny (the helper prints the deny; the
//! broker times out to deny).

pub mod escalation;
pub mod policy;
pub mod screening;
pub mod server;

pub use escalation::EscalationBroker;
pub use policy::{PolicyEngine, PolicyVerdict, WorkstreamCtx};
pub use screening::{ScreenResult, screen_order};
pub use server::{ControlServer, ControlServerHandle};
