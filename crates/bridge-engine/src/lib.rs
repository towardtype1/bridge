//! The engine: spawns and supervises `claude -p` child processes.

pub mod budget;
pub mod preflight;
pub mod ratelimit;
pub mod runner;
pub mod session;

pub use budget::{BudgetLedger, BudgetStatus};
pub use preflight::PreflightReport;
pub use ratelimit::{RateLimitGate, RateLimitHit};
pub use runner::{ClaudeRunner, EngineError, ExitClass, TurnCtx, TurnOutcome};
pub use session::SessionRegistry;
