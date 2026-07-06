//! The Bridge GUI application.
//!
//! `bridge [--repo <path>] [--config <bridge.toml>] [--headless-smoke]`
//!
//! Boot order:
//! 1. tracing init; config load (default `bridge.toml` in --repo, else
//!    built-in defaults),
//! 2. open the Ship's Computer (per-repo db under the platform data dir),
//! 3. orphan sweep: kill recorded child pids, prune stale worktrees,
//! 4. engine preflight (version check -> CompatWarning event when outside
//!    the tested range; GUI shows the banner with "proceed anyway"),
//! 5. bind the control server; build PolicyEngine, EscalationBroker,
//!    ClaudeRunner, WorktreeManager, LiveDeps,
//! 6. spawn MissionController on a background tokio runtime,
//! 7. run the eframe app on the main thread (event bus in, commands out).
//!
//! `--headless-smoke` skips eframe and drives one scripted mini-mission
//! for CI/e2e verification (see tests + Phase C of the plan).

mod state;
mod ui;
mod wiring;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    todo!()
}
