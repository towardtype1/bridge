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

use crate::runner::{ExitClass, TurnOutcome};
use bridge_compat::{ApiErrorCategory, StreamEvent};
use chrono::{DateTime, Utc};
use std::sync::Mutex;
use std::time::Duration;

/// A failing turn faster than this counts as an "immediate" failure.
const IMMEDIATE_FAILURE_WINDOW: Duration = Duration::from_secs(10);
/// Consecutive immediate failures that trip the gate heuristically.
const IMMEDIATE_FAILURE_TRIP: u32 = 3;
/// First probe delay of the exponential backoff schedule.
const INITIAL_PROBE_SECS: u64 = 30;
/// Backoff cap: 15 minutes.
const MAX_PROBE_SECS: u64 = 900;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitHit {
    pub retry_at: Option<DateTime<Utc>>,
    /// Human-readable trigger description for the GUI banner.
    pub trigger: String,
}

#[derive(Debug, Default)]
struct GateInner {
    hit: Option<RateLimitHit>,
    consecutive_immediate_failures: u32,
}

pub struct RateLimitGate {
    inner: Mutex<GateInner>,
}

impl RateLimitGate {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(GateInner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateInner> {
        self.inner.lock().expect("rate limit gate lock poisoned")
    }

    /// Trip the gate. An already-tripped gate keeps its first hit, except
    /// that a later signal carrying a concrete `retry_at` upgrades a hit
    /// that had none.
    fn trip(&self, hit: RateLimitHit) {
        let mut inner = self.lock();
        match inner.hit.as_mut() {
            None => {
                tracing::warn!(trigger = %hit.trigger, retry_at = ?hit.retry_at, "rate limit gate tripped");
                inner.hit = Some(hit);
            }
            Some(existing) if existing.retry_at.is_none() && hit.retry_at.is_some() => {
                existing.retry_at = hit.retry_at;
                existing.trigger = hit.trigger;
            }
            Some(_) => {}
        }
    }

    /// Feed every stream event through; cheap.
    pub fn observe_event(&self, ev: &StreamEvent) {
        match ev {
            StreamEvent::RateLimit(rl) if rl.status != "allowed" => {
                self.trip(RateLimitHit {
                    retry_at: rl.resets_at.and_then(|s| DateTime::from_timestamp(s, 0)),
                    trigger: format!("rate_limit_event status \"{}\"", rl.status),
                });
            }
            StreamEvent::ApiRetry(retry)
                if matches!(
                    retry.error,
                    ApiErrorCategory::RateLimit | ApiErrorCategory::Overloaded
                ) =>
            {
                if retry.max_retries.is_some_and(|max| retry.attempt >= max) {
                    self.trip(RateLimitHit {
                        retry_at: None,
                        trigger: format!(
                            "api_retry exhausted ({:?}, attempt {}/{})",
                            retry.error,
                            retry.attempt,
                            retry.max_retries.unwrap_or(0)
                        ),
                    });
                }
            }
            StreamEvent::Result(res) if res.looks_rate_limited() => {
                self.trip(RateLimitHit {
                    retry_at: None,
                    trigger: format!("result looks rate limited (subtype \"{}\")", res.subtype),
                });
            }
            _ => {}
        }
    }

    /// Feed each finished turn; applies the repeated-immediate-failure
    /// heuristic (`spawn_to_exit` under 10s + is_error).
    pub fn observe_outcome(&self, outcome: &TurnOutcome, spawn_to_exit: Duration) {
        if let Some(hit) = &outcome.rate_limit {
            self.trip(hit.clone());
        }
        let failed = match &outcome.exit {
            ExitClass::NonZero(_) | ExitClass::SpawnFailed(_) => true,
            ExitClass::Success => outcome.result.as_ref().is_some_and(|r| r.is_error),
            // A timeout kill is a hang, not a rate-limit signature.
            ExitClass::TimedOut => false,
        };
        let mut inner = self.lock();
        if failed && spawn_to_exit < IMMEDIATE_FAILURE_WINDOW {
            inner.consecutive_immediate_failures += 1;
            if inner.consecutive_immediate_failures >= IMMEDIATE_FAILURE_TRIP {
                let count = inner.consecutive_immediate_failures;
                drop(inner);
                self.trip(RateLimitHit {
                    retry_at: None,
                    trigger: format!("{count} consecutive immediate turn failures"),
                });
            }
        } else {
            inner.consecutive_immediate_failures = 0;
        }
    }

    /// Some(hit) while paused.
    pub fn paused(&self) -> Option<RateLimitHit> {
        self.lock().hit.clone()
    }

    /// Next probe delay (exponential: 30s, 60s, 120s ... capped 15m), or
    /// the exact remaining time when `retry_at` is known. `attempt` is the
    /// zero-based probe counter.
    pub fn probe_delay(&self, attempt: u32, now: DateTime<Utc>) -> Duration {
        if let Some(hit) = self.lock().hit.as_ref()
            && let Some(retry_at) = hit.retry_at
        {
            let remaining = (retry_at - now).num_seconds();
            return Duration::from_secs(remaining.max(0) as u64);
        }
        // 30 * 2^5 = 960 already exceeds the cap, so clamp the exponent
        // before shifting to avoid silent bit loss on large attempts.
        let secs = if attempt >= 5 {
            MAX_PROBE_SECS
        } else {
            (INITIAL_PROBE_SECS << attempt).min(MAX_PROBE_SECS)
        };
        Duration::from_secs(secs)
    }

    /// A probe succeeded: clear the gate.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.hit = None;
        inner.consecutive_immediate_failures = 0;
    }
}

impl Default for RateLimitGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::ExitClass;
    use bridge_compat::{ApiErrorCategory, ApiRetry, RateLimitEvent, ResultEvent};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap()
    }

    fn rate_limit_event(status: &str, resets_at: Option<i64>) -> StreamEvent {
        StreamEvent::RateLimit(RateLimitEvent {
            status: status.into(),
            resets_at,
            rate_limit_type: Some("five_hour".into()),
        })
    }

    fn api_retry(
        category: ApiErrorCategory,
        attempt: u32,
        max_retries: Option<u32>,
    ) -> StreamEvent {
        StreamEvent::ApiRetry(ApiRetry {
            attempt,
            max_retries,
            retry_delay_ms: Some(1000),
            error_status: Some(429),
            error: category,
        })
    }

    fn error_result() -> ResultEvent {
        ResultEvent {
            subtype: "error_during_execution".into(),
            is_error: true,
            session_id: Some("s".into()),
            num_turns: Some(0),
            result: None,
            total_cost_usd: None,
            duration_ms: Some(500),
            api_error_status: None,
            stop_reason: None,
            terminal_reason: None,
            structured_output: None,
            usage: None,
            permission_denials: vec![],
        }
    }

    fn outcome(exit: ExitClass, result: Option<ResultEvent>) -> TurnOutcome {
        TurnOutcome {
            exit,
            result,
            rate_limit: None,
            structured_output: None,
        }
    }

    #[test]
    fn fresh_gate_is_not_paused() {
        assert_eq!(RateLimitGate::new().paused(), None);
    }

    #[test]
    fn rejected_rate_limit_event_trips_with_retry_at() {
        let gate = RateLimitGate::new();
        gate.observe_event(&rate_limit_event("rejected", Some(1785542400)));
        let hit = gate.paused().expect("gate should be tripped");
        assert_eq!(hit.retry_at, DateTime::from_timestamp(1785542400, 0));
        assert!(hit.trigger.contains("rejected"), "trigger: {}", hit.trigger);
    }

    #[test]
    fn allowed_rate_limit_event_does_not_trip() {
        let gate = RateLimitGate::new();
        gate.observe_event(&rate_limit_event("allowed", Some(1785542400)));
        assert_eq!(gate.paused(), None);
    }

    #[test]
    fn api_retry_rate_limit_with_exhausted_retries_trips() {
        let gate = RateLimitGate::new();
        gate.observe_event(&api_retry(ApiErrorCategory::RateLimit, 3, Some(3)));
        assert!(gate.paused().is_some());
    }

    #[test]
    fn api_retry_overloaded_with_exhausted_retries_trips() {
        let gate = RateLimitGate::new();
        gate.observe_event(&api_retry(ApiErrorCategory::Overloaded, 5, Some(5)));
        assert!(gate.paused().is_some());
    }

    #[test]
    fn api_retry_with_attempts_remaining_does_not_trip() {
        let gate = RateLimitGate::new();
        gate.observe_event(&api_retry(ApiErrorCategory::RateLimit, 1, Some(3)));
        assert_eq!(gate.paused(), None);
    }

    #[test]
    fn api_retry_other_categories_never_trip() {
        let gate = RateLimitGate::new();
        gate.observe_event(&api_retry(ApiErrorCategory::ServerError, 9, Some(3)));
        gate.observe_event(&api_retry(ApiErrorCategory::BillingError, 9, Some(3)));
        assert_eq!(gate.paused(), None);
    }

    #[test]
    fn rate_limited_looking_result_trips() {
        let gate = RateLimitGate::new();
        let mut res = error_result();
        res.api_error_status = Some(429);
        gate.observe_event(&StreamEvent::Result(res));
        assert!(gate.paused().is_some());
    }

    #[test]
    fn three_consecutive_immediate_failures_trip() {
        let gate = RateLimitGate::new();
        let fast = Duration::from_secs(2);
        for i in 0..2 {
            gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
            assert_eq!(gate.paused(), None, "tripped too early at failure {i}");
        }
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        assert!(gate.paused().is_some());
    }

    #[test]
    fn success_resets_the_immediate_failure_counter() {
        let gate = RateLimitGate::new();
        let fast = Duration::from_secs(2);
        let mut ok = error_result();
        ok.is_error = false;
        ok.subtype = "success".into();
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        gate.observe_outcome(&outcome(ExitClass::Success, Some(ok)), fast);
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        assert_eq!(gate.paused(), None);
    }

    #[test]
    fn slow_failures_do_not_count_as_immediate() {
        let gate = RateLimitGate::new();
        let slow = Duration::from_secs(60);
        for _ in 0..5 {
            gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), slow);
        }
        assert_eq!(gate.paused(), None);
    }

    #[test]
    fn outcome_carrying_a_rate_limit_hit_trips_directly() {
        let gate = RateLimitGate::new();
        let mut o = outcome(ExitClass::NonZero(1), Some(error_result()));
        o.rate_limit = Some(RateLimitHit {
            retry_at: DateTime::from_timestamp(1785542400, 0),
            trigger: "rate_limit_event status \"rejected\"".into(),
        });
        gate.observe_outcome(&o, Duration::from_secs(60));
        let hit = gate.paused().expect("gate should be tripped");
        assert_eq!(hit.retry_at, DateTime::from_timestamp(1785542400, 0));
    }

    #[test]
    fn probe_delay_backs_off_exponentially_with_cap() {
        let gate = RateLimitGate::new();
        // No retry_at known: pure exponential schedule.
        assert_eq!(gate.probe_delay(0, now()), Duration::from_secs(30));
        assert_eq!(gate.probe_delay(1, now()), Duration::from_secs(60));
        assert_eq!(gate.probe_delay(2, now()), Duration::from_secs(120));
        assert_eq!(gate.probe_delay(3, now()), Duration::from_secs(240));
        assert_eq!(gate.probe_delay(4, now()), Duration::from_secs(480));
        assert_eq!(gate.probe_delay(5, now()), Duration::from_secs(900));
        assert_eq!(gate.probe_delay(6, now()), Duration::from_secs(900));
        assert_eq!(gate.probe_delay(63, now()), Duration::from_secs(900));
    }

    #[test]
    fn probe_delay_uses_exact_remaining_time_when_retry_at_known() {
        let gate = RateLimitGate::new();
        let retry_at = now() + chrono::Duration::seconds(345);
        gate.observe_event(&rate_limit_event("rejected", Some(retry_at.timestamp())));
        assert_eq!(gate.probe_delay(0, now()), Duration::from_secs(345));
        assert_eq!(gate.probe_delay(4, now()), Duration::from_secs(345));
        // Once the reset instant has passed, probe immediately.
        assert_eq!(
            gate.probe_delay(0, retry_at + chrono::Duration::seconds(1)),
            Duration::ZERO
        );
    }

    #[test]
    fn clear_resets_pause_and_failure_counter() {
        let gate = RateLimitGate::new();
        gate.observe_event(&rate_limit_event("rejected", None));
        let fast = Duration::from_secs(1);
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        assert!(gate.paused().is_some());
        gate.clear();
        assert_eq!(gate.paused(), None);
        // Counter restarted: two more immediate failures are not enough.
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        gate.observe_outcome(&outcome(ExitClass::NonZero(1), Some(error_result())), fast);
        assert_eq!(gate.paused(), None);
    }

    #[test]
    fn later_retry_at_information_upgrades_an_unknown_retry_at() {
        let gate = RateLimitGate::new();
        gate.observe_event(&rate_limit_event("rejected", None));
        assert_eq!(gate.paused().unwrap().retry_at, None);
        gate.observe_event(&rate_limit_event("rejected", Some(1785542400)));
        assert_eq!(
            gate.paused().unwrap().retry_at,
            DateTime::from_timestamp(1785542400, 0)
        );
    }
}
