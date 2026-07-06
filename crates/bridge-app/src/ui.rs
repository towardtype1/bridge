//! egui panels. Rendering only: read `AppState`, emit `BridgeCommand`s.
//!
//! Layout:
//! - Header strip: mission state chip, budget bars (turns, wall clock,
//!   cost), rate-limit countdown banner (retry-at), compat warning banner
//!   with "proceed anyway", RED ALERT toggle (prominent, red when armed).
//! - Left sidebar: workstreams with status badges; click to select.
//! - Center: selected workstream detail - live agent output feed, tool
//!   calls, turn history, filed battle reports.
//! - Right: Tactical log (hook decisions, newest first, color by
//!   decision) and the merge queue with per-entry state.
//! - Modals: escalation queue (approve / deny-with-reason, countdown to
//!   fail-closed deny), merge confirmation (diff stat + summary,
//!   confirm/reject), flagged-workstream override.
//! - Bottom: mission input box (objective -> StartMission), wind
//!   down / shutdown buttons.
//!
//! Theme: restrained LCARS accents (amber/salmon on dark), standard egui
//! widgets; readability beats cosplay.

use crate::state::AppState;
use bridge_core::BridgeCommand;

/// Draw one frame; queued commands are drained by the caller.
pub fn draw(ctx: &egui::Context, state: &mut AppState, out_commands: &mut Vec<BridgeCommand>) {
    todo!()
}

// Re-export egui through eframe for the single import point.
pub use eframe::egui;
