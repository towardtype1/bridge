//! Integration tests for the Ship's Computer, exercised through the public
//! API only. Every record/query method is covered, plus migration
//! idempotence and persistence across close/reopen.

use bridge_computer::{ComputerError, ShipsComputer};
use bridge_core::{
    BattleReport, BridgeConfig, DecisionKind, DecisionSource, Finding, HookDecisionRecord,
    MissionId, MissionPlan, OrderId, SessionId, Severity, Station, TurnRecord, Verdict,
    WorkstreamId, WorkstreamSpec, WorkstreamStatus,
};
use chrono::Utc;
use std::path::{Path, PathBuf};

fn plan_with(workstream_count: usize) -> MissionPlan {
    let workstreams = (0..workstream_count)
        .map(|i| WorkstreamSpec {
            id: WorkstreamId::new(),
            slug: format!("ws-{i}"),
            title: format!("Workstream {i}"),
            description: format!("description {i}"),
            base_ref: "main".into(),
        })
        .collect();
    MissionPlan {
        mission_id: MissionId::new(),
        slug: "test-mission".into(),
        objective: "test objective".into(),
        workstreams,
        edges: vec![],
    }
}

fn turn(ws: WorkstreamId, station: Station, num_turns: u32, cost: Option<f64>) -> TurnRecord {
    TurnRecord {
        order: OrderId::new(),
        workstream: ws,
        station,
        session: Some(SessionId::from("sess-1")),
        started_at: Utc::now(),
        duration_ms: 1234,
        num_turns,
        is_error: false,
        subtype: "success".into(),
        total_cost_usd: cost,
        input_tokens: Some(100),
        output_tokens: Some(50),
    }
}

fn finding(title: &str, weakness_class: &str) -> Finding {
    Finding {
        severity: Severity::High,
        title: title.into(),
        description: "adversarial finding".into(),
        reproduction_command: "cargo test --test adversarial".into(),
        failing_test_path: Some("tests/adversarial/case.rs".into()),
        weakness_class: weakness_class.into(),
    }
}

fn report(ws: WorkstreamId, round: u32, findings: Vec<Finding>) -> BattleReport {
    let verdict = if findings.is_empty() {
        Verdict::Clean
    } else {
        Verdict::Breached
    };
    BattleReport {
        workstream: ws,
        round,
        verdict,
        findings,
    }
}

fn decision(ws: WorkstreamId) -> HookDecisionRecord {
    HookDecisionRecord {
        timestamp: Utc::now(),
        workstream: ws,
        hook_event: "PreToolUse".into(),
        tool_name: Some("Bash".into()),
        decision: DecisionKind::Deny,
        reason: Some("rm -rf outside worktree".into()),
        rule: "prime_directive.worktree_escape".into(),
        source: DecisionSource::PrimeDirective,
        latency_ms: 3,
    }
}

// -- migrations ---------------------------------------------------------------

#[test]
fn open_same_path_twice_is_idempotent_and_keeps_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.db");

    let db = ShipsComputer::open(&path).unwrap();
    let ws = WorkstreamId::new();
    db.register_child_pid(4242, ws).unwrap();
    drop(db);

    // Second open must re-run migrations without error and keep the data.
    let db = ShipsComputer::open(&path).unwrap();
    assert_eq!(db.recorded_child_pids().unwrap(), vec![4242]);
}

#[test]
fn file_database_uses_wal_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.db");
    let db = ShipsComputer::open(&path).unwrap();
    drop(db);

    // WAL is persistent in the database file, so a raw reopen observes it.
    let raw = rusqlite::Connection::open(&path).unwrap();
    let mode: String = raw
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
}

// -- mission lifecycle ---------------------------------------------------------

#[test]
fn mission_round_trips_with_latest_statuses() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let plan = plan_with(2);
    let cfg = BridgeConfig::default();
    let ws0 = plan.workstreams[0].id;
    let ws1 = plan.workstreams[1].id;

    db.record_mission(&plan, &cfg).unwrap();
    db.record_workstream_status(ws1, &WorkstreamStatus::Working)
        .unwrap();
    db.record_workstream_status(ws0, &WorkstreamStatus::Working)
        .unwrap();
    db.record_workstream_status(ws0, &WorkstreamStatus::ReadyToMerge)
        .unwrap();

    let persisted = db
        .persisted_mission()
        .unwrap()
        .expect("mission should be offered");
    assert_eq!(persisted.plan, plan);
    assert_eq!(persisted.config, cfg);
    // Latest status per workstream, in plan declaration order.
    assert_eq!(
        persisted.statuses,
        vec![
            (ws0, WorkstreamStatus::ReadyToMerge),
            (ws1, WorkstreamStatus::Working)
        ]
    );
}

#[test]
fn workstreams_without_recorded_status_are_omitted() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let plan = plan_with(2);
    db.record_mission(&plan, &BridgeConfig::default()).unwrap();
    db.record_workstream_status(plan.workstreams[1].id, &WorkstreamStatus::Pending)
        .unwrap();

    let persisted = db.persisted_mission().unwrap().unwrap();
    assert_eq!(
        persisted.statuses,
        vec![(plan.workstreams[1].id, WorkstreamStatus::Pending)]
    );
}

#[test]
fn completed_mission_is_not_offered_for_resume() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let plan = plan_with(1);
    db.record_mission(&plan, &BridgeConfig::default()).unwrap();
    db.record_mission_complete(plan.mission_id).unwrap();
    assert_eq!(db.persisted_mission().unwrap(), None);
}

#[test]
fn most_recent_incomplete_mission_wins() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let cfg = BridgeConfig::default();
    let first = plan_with(1);
    let second = plan_with(1);
    db.record_mission(&first, &cfg).unwrap();
    db.record_mission(&second, &cfg).unwrap();

    let persisted = db.persisted_mission().unwrap().unwrap();
    assert_eq!(persisted.plan.mission_id, second.mission_id);

    // Completing the newest surfaces the older incomplete one again.
    db.record_mission_complete(second.mission_id).unwrap();
    let persisted = db.persisted_mission().unwrap().unwrap();
    assert_eq!(persisted.plan.mission_id, first.mission_id);
}

#[test]
fn empty_database_has_no_persisted_mission() {
    let db = ShipsComputer::open_in_memory().unwrap();
    assert_eq!(db.persisted_mission().unwrap(), None);
}

#[test]
fn re_recording_a_mission_replaces_plan_and_config() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let mut plan = plan_with(1);
    let cfg = BridgeConfig::default();
    db.record_mission(&plan, &cfg).unwrap();

    plan.objective = "revised objective".into();
    let mut cfg2 = cfg.clone();
    cfg2.budgets.max_total_turns = 77;
    db.record_mission(&plan, &cfg2).unwrap();

    let persisted = db.persisted_mission().unwrap().unwrap();
    assert_eq!(persisted.plan.objective, "revised objective");
    assert_eq!(persisted.config.budgets.max_total_turns, 77);
}

// -- turns, usage, decisions -----------------------------------------------------

#[test]
fn usage_snapshot_accumulates_recorded_turns() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let plan = plan_with(2);
    let cfg = BridgeConfig::default();
    let mission = plan.mission_id;
    let ws0 = plan.workstreams[0].id;
    let ws1 = plan.workstreams[1].id;
    db.record_mission(&plan, &cfg).unwrap();

    db.record_turn(mission, &turn(ws1, Station::Helm, 3, Some(0.50)))
        .unwrap();
    db.record_turn(mission, &turn(ws1, Station::Helm, 2, None))
        .unwrap();
    db.record_turn(mission, &turn(ws0, Station::KobayashiMaru, 4, Some(1.25)))
        .unwrap();

    let snap = db.usage_snapshot(mission, &cfg).unwrap();
    assert_eq!(snap.mission, mission);
    assert_eq!(snap.total_turns, 9);
    assert_eq!(snap.max_total_turns, cfg.budgets.max_total_turns);
    assert!((snap.total_cost_usd - 1.75).abs() < 1e-9);
    assert_eq!(snap.max_wall_clock_secs, cfg.budgets.max_wall_clock_secs);
    // Wall clock is measured from record_mission time to now: small and non-negative.
    assert!(
        snap.wall_clock_secs < 60,
        "wall clock should be tiny, got {}",
        snap.wall_clock_secs
    );

    // Per-workstream totals, in first-turn-recorded order.
    assert_eq!(snap.per_workstream.len(), 2);
    assert_eq!(snap.per_workstream[0].workstream, ws1);
    assert_eq!(snap.per_workstream[0].turns, 5);
    assert_eq!(snap.per_workstream[1].workstream, ws0);
    assert_eq!(snap.per_workstream[1].turns, 4);
}

#[test]
fn usage_snapshot_with_no_turns_is_zeroed() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let plan = plan_with(1);
    let cfg = BridgeConfig::default();
    db.record_mission(&plan, &cfg).unwrap();

    let snap = db.usage_snapshot(plan.mission_id, &cfg).unwrap();
    assert_eq!(snap.total_turns, 0);
    assert_eq!(snap.per_workstream, vec![]);
    assert_eq!(snap.total_cost_usd, 0.0);
}

#[test]
fn usage_snapshot_for_unknown_mission_is_an_error_not_a_panic() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let err = db.usage_snapshot(MissionId::new(), &BridgeConfig::default());
    assert!(matches!(err, Err(ComputerError::Db(_))));
}

#[test]
fn turns_by_station_sums_cli_reported_turns() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let plan = plan_with(1);
    let mission = plan.mission_id;
    let ws = plan.workstreams[0].id;
    db.record_mission(&plan, &BridgeConfig::default()).unwrap();

    db.record_turn(mission, &turn(ws, Station::Helm, 3, None))
        .unwrap();
    db.record_turn(mission, &turn(ws, Station::Helm, 2, None))
        .unwrap();
    db.record_turn(mission, &turn(ws, Station::KobayashiMaru, 7, None))
        .unwrap();
    // A turn on another mission must not leak in.
    db.record_turn(MissionId::new(), &turn(ws, Station::Helm, 100, None))
        .unwrap();

    let by_station = db.turns_by_station(mission).unwrap();
    assert_eq!(
        by_station,
        vec![(Station::Helm, 5), (Station::KobayashiMaru, 7)]
    );
}

#[test]
fn hook_decisions_are_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.db");
    let db = ShipsComputer::open(&path).unwrap();
    let mission = MissionId::new();
    let ws = WorkstreamId::new();
    db.record_hook_decision(mission, &decision(ws)).unwrap();
    db.record_hook_decision(mission, &decision(ws)).unwrap();
    drop(db);

    let raw = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM hook_decisions WHERE mission_id = ?1 AND workstream_id = ?2",
            [mission.to_string(), ws.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

// -- sessions --------------------------------------------------------------------

#[test]
fn session_round_trips_and_upserts() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let ws = WorkstreamId::new();

    assert_eq!(db.session_for(ws, Station::Helm).unwrap(), None);

    db.record_session(
        ws,
        Station::Helm,
        &SessionId::from("sess-a"),
        Path::new("/tmp/wt-a"),
    )
    .unwrap();
    assert_eq!(
        db.session_for(ws, Station::Helm).unwrap(),
        Some((SessionId::from("sess-a"), PathBuf::from("/tmp/wt-a")))
    );

    // Upsert: same (workstream, station) key replaces both session and cwd.
    db.record_session(
        ws,
        Station::Helm,
        &SessionId::from("sess-b"),
        Path::new("/tmp/wt-b"),
    )
    .unwrap();
    assert_eq!(
        db.session_for(ws, Station::Helm).unwrap(),
        Some((SessionId::from("sess-b"), PathBuf::from("/tmp/wt-b")))
    );

    // A different station on the same workstream is a separate key.
    assert_eq!(db.session_for(ws, Station::KobayashiMaru).unwrap(), None);
}

// -- battle reports ------------------------------------------------------------------

#[test]
fn findings_are_indexed_by_files_and_returned_newest_report_first() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let ws = WorkstreamId::new();
    let a = PathBuf::from("src/a.rs");
    let b = PathBuf::from("src/b.rs");

    let f1 = finding("first-round finding", "path-traversal");
    let r1 = report(ws, 1, vec![f1.clone()]);
    db.record_battle_report(&r1, &[a.clone(), b.clone()])
        .unwrap();

    let f2 = finding("second-round finding A", "race-condition");
    let f3 = finding("second-round finding B", "unvalidated-external-content");
    let r2 = report(ws, 2, vec![f2.clone(), f3.clone()]);
    db.record_battle_report(&r2, std::slice::from_ref(&b))
        .unwrap();

    // b.rs intersects both reports: newest report first, findings in report order.
    let hits = db.findings_for_files(std::slice::from_ref(&b)).unwrap();
    assert_eq!(hits, vec![f2.clone(), f3.clone(), f1.clone()]);

    // a.rs only intersects the first report.
    let hits = db.findings_for_files(std::slice::from_ref(&a)).unwrap();
    assert_eq!(hits, vec![f1.clone()]);

    // Querying with both files must not duplicate findings.
    let hits = db.findings_for_files(&[a.clone(), b.clone()]).unwrap();
    assert_eq!(hits, vec![f2, f3, f1]);
}

#[test]
fn disjoint_files_return_no_findings() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let ws = WorkstreamId::new();
    let r = report(ws, 1, vec![finding("f", "misc")]);
    db.record_battle_report(&r, &[PathBuf::from("src/a.rs")])
        .unwrap();

    assert_eq!(
        db.findings_for_files(&[PathBuf::from("src/other.rs")])
            .unwrap(),
        vec![]
    );
    assert_eq!(db.findings_for_files(&[]).unwrap(), vec![]);
}

#[test]
fn clean_report_with_no_findings_is_recorded_without_error() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let r = report(WorkstreamId::new(), 1, vec![]);
    db.record_battle_report(&r, &[PathBuf::from("src/a.rs")])
        .unwrap();
    assert_eq!(
        db.findings_for_files(&[PathBuf::from("src/a.rs")]).unwrap(),
        vec![]
    );
}

#[test]
fn duplicate_file_entries_are_tolerated() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let ws = WorkstreamId::new();
    let a = PathBuf::from("src/a.rs");
    let f = finding("f", "misc");
    db.record_battle_report(&report(ws, 1, vec![f.clone()]), &[a.clone(), a.clone()])
        .unwrap();
    assert_eq!(db.findings_for_files(&[a.clone(), a]).unwrap(), vec![f]);
}

// -- child pid registry ------------------------------------------------------------------

#[test]
fn pid_registry_registers_clears_and_lists() {
    let db = ShipsComputer::open_in_memory().unwrap();
    let ws = WorkstreamId::new();

    assert_eq!(db.recorded_child_pids().unwrap(), Vec::<u32>::new());

    db.register_child_pid(300, ws).unwrap();
    db.register_child_pid(100, ws).unwrap();
    db.register_child_pid(200, ws).unwrap();
    assert_eq!(db.recorded_child_pids().unwrap(), vec![100, 200, 300]);

    db.clear_child_pid(200).unwrap();
    assert_eq!(db.recorded_child_pids().unwrap(), vec![100, 300]);

    // Re-registering an existing pid (recycled by the OS) is an upsert.
    db.register_child_pid(100, WorkstreamId::new()).unwrap();
    assert_eq!(db.recorded_child_pids().unwrap(), vec![100, 300]);

    // Clearing an unknown pid is a no-op, not an error.
    db.clear_child_pid(9999).unwrap();
}

// -- persistence across close/reopen ---------------------------------------------------

#[test]
fn everything_persists_across_close_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.db");
    let plan = plan_with(1);
    let cfg = BridgeConfig::default();
    let mission = plan.mission_id;
    let ws = plan.workstreams[0].id;
    let f = finding("persisted finding", "path-traversal");

    {
        let db = ShipsComputer::open(&path).unwrap();
        db.record_mission(&plan, &cfg).unwrap();
        db.record_workstream_status(ws, &WorkstreamStatus::Working)
            .unwrap();
        db.record_turn(mission, &turn(ws, Station::Helm, 2, Some(0.25)))
            .unwrap();
        db.record_session(
            ws,
            Station::Helm,
            &SessionId::from("sess-x"),
            Path::new("/tmp/wt"),
        )
        .unwrap();
        db.record_battle_report(
            &report(ws, 1, vec![f.clone()]),
            &[PathBuf::from("src/a.rs")],
        )
        .unwrap();
        db.register_child_pid(555, ws).unwrap();
    }

    let db = ShipsComputer::open(&path).unwrap();
    let persisted = db.persisted_mission().unwrap().unwrap();
    assert_eq!(persisted.plan, plan);
    assert_eq!(persisted.statuses, vec![(ws, WorkstreamStatus::Working)]);
    assert_eq!(persisted.config, cfg);

    let snap = db.usage_snapshot(mission, &cfg).unwrap();
    assert_eq!(snap.total_turns, 2);
    assert!((snap.total_cost_usd - 0.25).abs() < 1e-9);

    assert_eq!(
        db.session_for(ws, Station::Helm).unwrap(),
        Some((SessionId::from("sess-x"), PathBuf::from("/tmp/wt")))
    );
    assert_eq!(
        db.findings_for_files(&[PathBuf::from("src/a.rs")]).unwrap(),
        vec![f]
    );
    assert_eq!(db.recorded_child_pids().unwrap(), vec![555]);
}
