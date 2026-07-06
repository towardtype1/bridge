//! Budget accounting and enforcement (owned by Ops, mechanics live here).

use bridge_core::{
    BudgetConfig, BudgetExtension, BudgetSnapshot, MissionId, WorkstreamId, WorkstreamTurns,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetStatus {
    Ok,
    /// Which ceiling was hit, e.g. "max_total_turns",
    /// "max_turns_per_workstream:<slug>", "max_wall_clock_secs".
    Exhausted {
        which: String,
    },
}

#[derive(Debug)]
struct LedgerInner {
    /// Live ceilings; raised by `extend()`.
    max_total_turns: u32,
    max_turns_per_workstream: u32,
    max_wall_clock_secs: Option<u64>,
    total_turns: u32,
    per_workstream: HashMap<WorkstreamId, u32>,
    kobayashi_rounds: HashMap<WorkstreamId, u32>,
    total_cost_usd: f64,
}

/// Thread-safe ledger. Time is injected so tests are deterministic.
pub struct BudgetLedger {
    mission: MissionId,
    started_at: DateTime<Utc>,
    inner: Mutex<LedgerInner>,
}

impl BudgetLedger {
    pub fn new(mission: MissionId, config: BudgetConfig, started_at: DateTime<Utc>) -> Self {
        Self {
            mission,
            started_at,
            inner: Mutex::new(LedgerInner {
                max_total_turns: config.max_total_turns,
                max_turns_per_workstream: config.max_turns_per_workstream,
                max_wall_clock_secs: config.max_wall_clock_secs,
                total_turns: 0,
                per_workstream: HashMap::new(),
                kobayashi_rounds: HashMap::new(),
                total_cost_usd: 0.0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LedgerInner> {
        self.inner.lock().expect("budget ledger lock poisoned")
    }

    /// Record a completed turn (any station) against a workstream.
    pub fn record_turn(&self, ws: WorkstreamId, cost_usd: Option<f64>) {
        let mut inner = self.lock();
        inner.total_turns += 1;
        *inner.per_workstream.entry(ws).or_insert(0) += 1;
        if let Some(cost) = cost_usd {
            inner.total_cost_usd += cost;
        }
    }

    /// Kobayashi rounds are budgeted separately per workstream.
    /// Returns the new round count for `ws`.
    pub fn record_kobayashi_round(&self, ws: WorkstreamId) -> u32 {
        let mut inner = self.lock();
        let rounds = inner.kobayashi_rounds.entry(ws).or_insert(0);
        *rounds += 1;
        *rounds
    }

    pub fn kobayashi_rounds(&self, ws: WorkstreamId) -> u32 {
        self.lock().kobayashi_rounds.get(&ws).copied().unwrap_or(0)
    }

    /// Check BEFORE dispatching a turn; `now` injected for testability.
    pub fn check(&self, ws: WorkstreamId, now: DateTime<Utc>) -> BudgetStatus {
        let inner = self.lock();
        if inner.total_turns >= inner.max_total_turns {
            return BudgetStatus::Exhausted {
                which: "max_total_turns".into(),
            };
        }
        let ws_turns = inner.per_workstream.get(&ws).copied().unwrap_or(0);
        if ws_turns >= inner.max_turns_per_workstream {
            return BudgetStatus::Exhausted {
                which: format!("max_turns_per_workstream:{ws}"),
            };
        }
        if let Some(max_secs) = inner.max_wall_clock_secs
            && Self::elapsed_secs(self.started_at, now) >= max_secs
        {
            return BudgetStatus::Exhausted {
                which: "max_wall_clock_secs".into(),
            };
        }
        BudgetStatus::Ok
    }

    pub fn extend(&self, ext: BudgetExtension) {
        let mut inner = self.lock();
        inner.max_total_turns += ext.extra_total_turns;
        inner.max_turns_per_workstream += ext.extra_turns_per_workstream;
        if let Some(max_secs) = inner.max_wall_clock_secs.as_mut() {
            *max_secs += ext.extra_wall_clock_secs;
        }
    }

    pub fn snapshot(&self, now: DateTime<Utc>) -> BudgetSnapshot {
        let inner = self.lock();
        let mut per_workstream: Vec<WorkstreamTurns> = inner
            .per_workstream
            .iter()
            .map(|(ws, turns)| WorkstreamTurns {
                workstream: *ws,
                turns: *turns,
            })
            .collect();
        per_workstream.sort_by_key(|w| w.workstream);
        BudgetSnapshot {
            mission: self.mission,
            total_turns: inner.total_turns,
            max_total_turns: inner.max_total_turns,
            per_workstream,
            wall_clock_secs: Self::elapsed_secs(self.started_at, now),
            max_wall_clock_secs: inner.max_wall_clock_secs,
            total_cost_usd: inner.total_cost_usd,
        }
    }

    fn elapsed_secs(started_at: DateTime<Utc>, now: DateTime<Utc>) -> u64 {
        (now - started_at).num_seconds().max(0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap()
    }

    fn config(total: u32, per_ws: u32, wall: Option<u64>) -> BudgetConfig {
        BudgetConfig {
            max_total_turns: total,
            max_turns_per_workstream: per_ws,
            max_kobayashi_rounds: 3,
            max_wall_clock_secs: wall,
        }
    }

    fn ledger(total: u32, per_ws: u32, wall: Option<u64>) -> BudgetLedger {
        BudgetLedger::new(MissionId::new(), config(total, per_ws, wall), t0())
    }

    #[test]
    fn fresh_ledger_is_ok() {
        let l = ledger(10, 5, None);
        assert_eq!(l.check(WorkstreamId::new(), t0()), BudgetStatus::Ok);
    }

    #[test]
    fn total_turn_ceiling_exhausts() {
        let l = ledger(2, 10, None);
        let ws = WorkstreamId::new();
        l.record_turn(ws, Some(0.01));
        assert_eq!(l.check(ws, t0()), BudgetStatus::Ok);
        l.record_turn(ws, Some(0.01));
        assert_eq!(
            l.check(ws, t0()),
            BudgetStatus::Exhausted {
                which: "max_total_turns".into()
            }
        );
    }

    #[test]
    fn per_workstream_ceiling_exhausts_only_that_workstream() {
        let l = ledger(100, 2, None);
        let ws_a = WorkstreamId::new();
        let ws_b = WorkstreamId::new();
        l.record_turn(ws_a, None);
        l.record_turn(ws_a, None);
        assert_eq!(
            l.check(ws_a, t0()),
            BudgetStatus::Exhausted {
                which: format!("max_turns_per_workstream:{ws_a}")
            }
        );
        assert_eq!(l.check(ws_b, t0()), BudgetStatus::Ok);
    }

    #[test]
    fn wall_clock_ceiling_uses_injected_now() {
        let l = ledger(100, 100, Some(3600));
        let ws = WorkstreamId::new();
        assert_eq!(
            l.check(ws, t0() + chrono::Duration::seconds(3599)),
            BudgetStatus::Ok
        );
        assert_eq!(
            l.check(ws, t0() + chrono::Duration::seconds(3600)),
            BudgetStatus::Exhausted {
                which: "max_wall_clock_secs".into()
            }
        );
    }

    #[test]
    fn no_wall_clock_ceiling_never_exhausts_on_time() {
        let l = ledger(100, 100, None);
        assert_eq!(
            l.check(WorkstreamId::new(), t0() + chrono::Duration::days(365)),
            BudgetStatus::Ok
        );
    }

    #[test]
    fn extend_raises_ceilings() {
        let l = ledger(1, 1, Some(60));
        let ws = WorkstreamId::new();
        l.record_turn(ws, None);
        assert!(matches!(l.check(ws, t0()), BudgetStatus::Exhausted { .. }));
        l.extend(BudgetExtension {
            extra_total_turns: 5,
            extra_turns_per_workstream: 5,
            extra_wall_clock_secs: 3600,
        });
        assert_eq!(
            l.check(ws, t0() + chrono::Duration::seconds(120)),
            BudgetStatus::Ok
        );
        let snap = l.snapshot(t0());
        assert_eq!(snap.max_total_turns, 6);
        assert_eq!(snap.max_wall_clock_secs, Some(3660));
    }

    #[test]
    fn extend_with_zero_fields_changes_nothing() {
        let l = ledger(3, 2, None);
        l.extend(BudgetExtension::default());
        let snap = l.snapshot(t0());
        assert_eq!(snap.max_total_turns, 3);
        assert_eq!(snap.max_wall_clock_secs, None);
    }

    #[test]
    fn kobayashi_rounds_count_per_workstream() {
        let l = ledger(100, 100, None);
        let ws_a = WorkstreamId::new();
        let ws_b = WorkstreamId::new();
        assert_eq!(l.kobayashi_rounds(ws_a), 0);
        assert_eq!(l.record_kobayashi_round(ws_a), 1);
        assert_eq!(l.record_kobayashi_round(ws_a), 2);
        assert_eq!(l.record_kobayashi_round(ws_b), 1);
        assert_eq!(l.kobayashi_rounds(ws_a), 2);
        assert_eq!(l.kobayashi_rounds(ws_b), 1);
    }

    #[test]
    fn kobayashi_rounds_do_not_consume_turn_budget() {
        let l = ledger(100, 1, None);
        let ws = WorkstreamId::new();
        l.record_kobayashi_round(ws);
        l.record_kobayashi_round(ws);
        assert_eq!(l.check(ws, t0()), BudgetStatus::Ok);
    }

    #[test]
    fn snapshot_reports_usage() {
        let mission = MissionId::new();
        let l = BudgetLedger::new(mission, config(10, 5, Some(600)), t0());
        let ws_a = WorkstreamId::new();
        let ws_b = WorkstreamId::new();
        l.record_turn(ws_a, Some(0.25));
        l.record_turn(ws_a, None);
        l.record_turn(ws_b, Some(0.5));
        let snap = l.snapshot(t0() + chrono::Duration::seconds(42));
        assert_eq!(snap.mission, mission);
        assert_eq!(snap.total_turns, 3);
        assert_eq!(snap.max_total_turns, 10);
        assert_eq!(snap.wall_clock_secs, 42);
        assert_eq!(snap.max_wall_clock_secs, Some(600));
        assert!((snap.total_cost_usd - 0.75).abs() < 1e-9);
        assert_eq!(snap.per_workstream.len(), 2);
        let turns_a = snap
            .per_workstream
            .iter()
            .find(|w| w.workstream == ws_a)
            .unwrap()
            .turns;
        assert_eq!(turns_a, 2);
    }

    #[test]
    fn snapshot_before_start_clamps_wall_clock_to_zero() {
        let l = ledger(10, 5, None);
        let snap = l.snapshot(t0() - chrono::Duration::seconds(5));
        assert_eq!(snap.wall_clock_secs, 0);
    }
}
