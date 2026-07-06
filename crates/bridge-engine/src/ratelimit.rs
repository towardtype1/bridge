//! Subscription rate-limit detection and pause/probe scheduling.
//!
//! Signals (any one trips the gate):
//! - a `rate_limit_event` with status != "allowed" (its `resets_at` gives
//!   the retry-at hint),
//! - `system/api_retry` events with category RateLimit/Overloaded that
//!   exhaust their retries (result then reports an error),
//! - a `result` where `looks_rate_limited()` is true,
//! - N (default 3) consecutive turns failing within a few seconds of
//!   spawn (repeated immediate failures = limit-hit heuristic).
//!
//! On trip: the mission PAUSES (children are NOT killed; sessions resume
//! later via --resume). Recovery uses exponential backoff probes starting
//! at `initial_probe_secs`, doubling to a cap, or a single timer when
//! `retry_at` is known.

use bridge_compat::StreamEvent;
use crate::runner::TurnOutcome;
use chrono::{DateTime, Utc};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitHit {
    pub retry_at: Option<DateTime<Utc>>,
    /// Human-readable trigger description for the GUI banner.
    pub trigger: String,
}

pub struct RateLimitGate {
    _priv: (),
}

impl RateLimitGate {
    pub fn new() -> Self {
        todo!()
    }

    /// Feed every stream event through; cheap.
    pub fn observe_event(&self, ev: &StreamEvent) {
        todo!()
    }

    /// Feed each finished turn; applies the repeated-immediate-failure
    /// heuristic (`spawn_to_exit` under 10s + is_error).
    pub fn observe_outcome(&self, outcome: &TurnOutcome, spawn_to_exit: Duration) {
        todo!()
    }

    /// Some(hit) while paused.
    pub fn paused(&self) -> Option<RateLimitHit> {
        todo!()
    }

    /// Next probe delay (exponential: 30s, 60s, 120s ... capped 15m), or
    /// the exact remaining time when `retry_at` is known. `attempt` is the
    /// zero-based probe counter.
    pub fn probe_delay(&self, attempt: u32, now: DateTime<Utc>) -> Duration {
        todo!()
    }

    /// A probe succeeded: clear the gate.
    pub fn clear(&self) {
        todo!()
    }
}
