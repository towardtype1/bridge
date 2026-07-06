//! The Ship's Computer: SQLite persistence for everything worth remembering.
//!
//! One database per target repository (path chosen by the app). WAL mode.
//! All methods take `&self`; internal Mutex<Connection> keeps this Send +
//! Sync so stations can share an Arc<ShipsComputer>.

use bridge_core::{
    BattleReport, BridgeConfig, BudgetSnapshot, Finding, HookDecisionRecord, MissionId,
    MissionPlan, SessionId, Severity, Station, TurnRecord, Verdict, WorkstreamId, WorkstreamStatus,
    WorkstreamTurns,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComputerError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// A mission as persisted for resume-after-restart.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedMission {
    pub plan: MissionPlan,
    pub statuses: Vec<(WorkstreamId, WorkstreamStatus)>,
    /// Config snapshot taken when the mission started.
    pub config: BridgeConfig,
}

/// Schema, applied when `PRAGMA user_version` is behind. Rich payloads are
/// stored as JSON columns beside the indexed key columns used by queries.
const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS missions (
    mission_id      TEXT PRIMARY KEY,
    plan_json       TEXT NOT NULL,
    config_json     TEXT NOT NULL,
    started_at_ms   INTEGER NOT NULL,
    completed_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS workstream_statuses (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    workstream_id  TEXT NOT NULL,
    status_json    TEXT NOT NULL,
    recorded_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ws_statuses_ws ON workstream_statuses(workstream_id, id);

CREATE TABLE IF NOT EXISTS turns (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    mission_id     TEXT NOT NULL,
    workstream_id  TEXT NOT NULL,
    station        TEXT NOT NULL,
    started_at_ms  INTEGER NOT NULL,
    num_turns      INTEGER NOT NULL,
    total_cost_usd REAL,
    turn_json      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_turns_mission_ws ON turns(mission_id, workstream_id);
CREATE INDEX IF NOT EXISTS idx_turns_mission_station ON turns(mission_id, station);

CREATE TABLE IF NOT EXISTS hook_decisions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    mission_id    TEXT NOT NULL,
    workstream_id TEXT NOT NULL,
    hook_event    TEXT NOT NULL,
    timestamp_ms  INTEGER NOT NULL,
    decision_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_hook_decisions_mission ON hook_decisions(mission_id, workstream_id);

CREATE TABLE IF NOT EXISTS sessions (
    workstream_id TEXT NOT NULL,
    station       TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    cwd           TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (workstream_id, station)
);

CREATE TABLE IF NOT EXISTS battle_reports (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    workstream_id TEXT NOT NULL,
    round         INTEGER NOT NULL,
    verdict       TEXT NOT NULL,
    filed_at_ms   INTEGER NOT NULL,
    report_json   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_battle_reports_ws ON battle_reports(workstream_id, id);

CREATE TABLE IF NOT EXISTS findings (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    report_id      INTEGER NOT NULL REFERENCES battle_reports(id) ON DELETE CASCADE,
    severity       TEXT NOT NULL,
    weakness_class TEXT NOT NULL,
    finding_json   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_findings_report ON findings(report_id);
CREATE INDEX IF NOT EXISTS idx_findings_weakness ON findings(weakness_class);

CREATE TABLE IF NOT EXISTS findings_files (
    finding_id INTEGER NOT NULL REFERENCES findings(id) ON DELETE CASCADE,
    file_path  TEXT NOT NULL,
    PRIMARY KEY (finding_id, file_path)
);
CREATE INDEX IF NOT EXISTS idx_findings_files_path ON findings_files(file_path);

CREATE TABLE IF NOT EXISTS child_pids (
    pid              INTEGER PRIMARY KEY,
    workstream_id    TEXT NOT NULL,
    registered_at_ms INTEGER NOT NULL
);
";

const SCHEMA_VERSION: i64 = 1;

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn clamp_u32(v: i64) -> u32 {
    v.clamp(0, i64::from(u32::MAX)) as u32
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Clean => "clean",
        Verdict::Breached => "breached",
    }
}

fn station_from_str(s: &str) -> Option<Station> {
    Station::ALL.iter().copied().find(|st| st.as_str() == s)
}

pub struct ShipsComputer {
    conn: Mutex<Connection>,
}

impl ShipsComputer {
    /// Open (creating if needed) and migrate. Migrations are idempotent and
    /// versioned via `PRAGMA user_version`.
    pub fn open(db_path: &Path) -> Result<Self, ComputerError> {
        Self::init(Connection::open(db_path)?)
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> Result<Self, ComputerError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, ComputerError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL persists in the file header; in-memory databases report
        // "memory" here, which is fine.
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn migrate(conn: &Connection) -> Result<(), ComputerError> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(())
    }

    /// A poisoned mutex only means another thread panicked mid-query; the
    /// connection itself is still usable, so recover it rather than panic.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // -- mission lifecycle ---------------------------------------------------

    /// Insert or replace the mission plan + config snapshot. Re-recording an
    /// existing mission updates the plan and config but preserves the
    /// original start time (the wall-clock anchor) and completion mark.
    pub fn record_mission(
        &self,
        plan: &MissionPlan,
        config: &BridgeConfig,
    ) -> Result<(), ComputerError> {
        let plan_json = serde_json::to_string(plan)?;
        let config_json = serde_json::to_string(config)?;
        self.lock().execute(
            "INSERT INTO missions (mission_id, plan_json, config_json, started_at_ms, completed_at_ms)
             VALUES (?1, ?2, ?3, ?4, NULL)
             ON CONFLICT(mission_id) DO UPDATE SET
                 plan_json = excluded.plan_json,
                 config_json = excluded.config_json",
            params![plan.mission_id.to_string(), plan_json, config_json, now_ms()],
        )?;
        Ok(())
    }

    pub fn record_workstream_status(
        &self,
        ws: WorkstreamId,
        status: &WorkstreamStatus,
    ) -> Result<(), ComputerError> {
        let status_json = serde_json::to_string(status)?;
        self.lock().execute(
            "INSERT INTO workstream_statuses (workstream_id, status_json, recorded_at_ms)
             VALUES (?1, ?2, ?3)",
            params![ws.to_string(), status_json, now_ms()],
        )?;
        Ok(())
    }

    pub fn record_mission_complete(&self, mission: MissionId) -> Result<(), ComputerError> {
        self.lock().execute(
            "UPDATE missions SET completed_at_ms = ?2 WHERE mission_id = ?1",
            params![mission.to_string(), now_ms()],
        )?;
        Ok(())
    }

    /// The most recent incomplete mission, if any (offer resume at startup).
    /// Statuses are the latest recorded per workstream, in plan declaration
    /// order; workstreams that never had a status recorded are omitted.
    pub fn persisted_mission(&self) -> Result<Option<PersistedMission>, ComputerError> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT plan_json, config_json FROM missions
                 WHERE completed_at_ms IS NULL
                 ORDER BY started_at_ms DESC, rowid DESC
                 LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((plan_json, config_json)) = row else {
            return Ok(None);
        };
        let plan: MissionPlan = serde_json::from_str(&plan_json)?;
        let config: BridgeConfig = serde_json::from_str(&config_json)?;

        let mut statuses = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT status_json FROM workstream_statuses
             WHERE workstream_id = ?1
             ORDER BY id DESC
             LIMIT 1",
        )?;
        for ws in &plan.workstreams {
            let latest: Option<String> = stmt
                .query_row([ws.id.to_string()], |r| r.get(0))
                .optional()?;
            if let Some(json) = latest {
                statuses.push((ws.id, serde_json::from_str(&json)?));
            }
        }
        drop(stmt);
        Ok(Some(PersistedMission {
            plan,
            statuses,
            config,
        }))
    }

    // -- turns, usage, decisions ----------------------------------------------

    pub fn record_turn(&self, mission: MissionId, turn: &TurnRecord) -> Result<(), ComputerError> {
        let turn_json = serde_json::to_string(turn)?;
        self.lock().execute(
            "INSERT INTO turns
                 (mission_id, workstream_id, station, started_at_ms, num_turns, total_cost_usd, turn_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                mission.to_string(),
                turn.workstream.to_string(),
                turn.station.as_str(),
                turn.started_at.timestamp_millis(),
                i64::from(turn.num_turns),
                turn.total_cost_usd,
                turn_json,
            ],
        )?;
        Ok(())
    }

    pub fn record_hook_decision(
        &self,
        mission: MissionId,
        d: &HookDecisionRecord,
    ) -> Result<(), ComputerError> {
        let decision_json = serde_json::to_string(d)?;
        self.lock().execute(
            "INSERT INTO hook_decisions
                 (mission_id, workstream_id, hook_event, timestamp_ms, decision_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                mission.to_string(),
                d.workstream.to_string(),
                d.hook_event,
                d.timestamp.timestamp_millis(),
                decision_json,
            ],
        )?;
        Ok(())
    }

    /// Live budget usage derived from recorded turns + mission start time.
    /// Turn totals sum the CLI-reported `num_turns` of each invocation; the
    /// wall clock runs from `record_mission` time to now. Errors if the
    /// mission was never recorded.
    pub fn usage_snapshot(
        &self,
        mission: MissionId,
        cfg: &BridgeConfig,
    ) -> Result<BudgetSnapshot, ComputerError> {
        let conn = self.lock();
        let started_at_ms: i64 = conn.query_row(
            "SELECT started_at_ms FROM missions WHERE mission_id = ?1",
            [mission.to_string()],
            |r| r.get(0),
        )?;
        let (total_turns, total_cost_usd): (i64, f64) = conn.query_row(
            "SELECT COALESCE(SUM(num_turns), 0), COALESCE(SUM(total_cost_usd), 0.0)
             FROM turns WHERE mission_id = ?1",
            [mission.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let mut per_workstream = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT workstream_id, SUM(num_turns) FROM turns
             WHERE mission_id = ?1
             GROUP BY workstream_id
             ORDER BY MIN(id)",
        )?;
        let rows = stmt.query_map([mission.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (ws_str, turns) = row?;
            match ws_str.parse::<WorkstreamId>() {
                Ok(workstream) => {
                    per_workstream.push(WorkstreamTurns {
                        workstream,
                        turns: clamp_u32(turns),
                    });
                }
                Err(err) => {
                    tracing::warn!("skipping malformed workstream id {ws_str:?} in turns: {err}");
                }
            }
        }

        let elapsed_ms = (now_ms() - started_at_ms).max(0);
        Ok(BudgetSnapshot {
            mission,
            total_turns: clamp_u32(total_turns),
            max_total_turns: cfg.budgets.max_total_turns,
            per_workstream,
            wall_clock_secs: (elapsed_ms / 1000) as u64,
            max_wall_clock_secs: cfg.budgets.max_wall_clock_secs,
            total_cost_usd,
        })
    }

    /// Turn counts per station for the GUI header: sums the CLI-reported
    /// `num_turns` per station, returned in `Station::ALL` order for the
    /// stations that have any turns.
    pub fn turns_by_station(
        &self,
        mission: MissionId,
    ) -> Result<Vec<(Station, u32)>, ComputerError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT station, SUM(num_turns) FROM turns WHERE mission_id = ?1 GROUP BY station",
        )?;
        let rows = stmt.query_map([mission.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut by_station: HashMap<Station, i64> = HashMap::new();
        for row in rows {
            let (name, turns) = row?;
            match station_from_str(&name) {
                Some(station) => {
                    by_station.insert(station, turns);
                }
                None => tracing::warn!("skipping turns recorded under unknown station {name:?}"),
            }
        }
        let mut out = Vec::new();
        for station in Station::ALL {
            if let Some(turns) = by_station.remove(&station) {
                out.push((station, clamp_u32(turns)));
            }
        }
        Ok(out)
    }

    // -- sessions --------------------------------------------------------------

    /// Persist the claude session for (workstream, station) with the cwd it
    /// must be resumed from. Upsert.
    pub fn record_session(
        &self,
        ws: WorkstreamId,
        station: Station,
        session: &SessionId,
        cwd: &Path,
    ) -> Result<(), ComputerError> {
        self.lock().execute(
            "INSERT INTO sessions (workstream_id, station, session_id, cwd, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workstream_id, station) DO UPDATE SET
                 session_id = excluded.session_id,
                 cwd = excluded.cwd,
                 updated_at_ms = excluded.updated_at_ms",
            params![
                ws.to_string(),
                station.as_str(),
                session.0,
                path_str(cwd),
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn session_for(
        &self,
        ws: WorkstreamId,
        station: Station,
    ) -> Result<Option<(SessionId, PathBuf)>, ComputerError> {
        let found = self
            .lock()
            .query_row(
                "SELECT session_id, cwd FROM sessions WHERE workstream_id = ?1 AND station = ?2",
                params![ws.to_string(), station.as_str()],
                |r| {
                    Ok((
                        SessionId(r.get::<_, String>(0)?),
                        PathBuf::from(r.get::<_, String>(1)?),
                    ))
                },
            )
            .optional()?;
        Ok(found)
    }

    // -- battle reports ----------------------------------------------------------

    /// Store a report and index findings by touched file paths extracted
    /// from `files` (caller supplies the diff file list of the workstream).
    pub fn record_battle_report(
        &self,
        report: &BattleReport,
        files: &[PathBuf],
    ) -> Result<(), ComputerError> {
        let report_json = serde_json::to_string(report)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO battle_reports (workstream_id, round, verdict, filed_at_ms, report_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                report.workstream.to_string(),
                i64::from(report.round),
                verdict_str(report.verdict),
                now_ms(),
                report_json,
            ],
        )?;
        let report_id = tx.last_insert_rowid();
        {
            let mut insert_finding = tx.prepare(
                "INSERT INTO findings (report_id, severity, weakness_class, finding_json)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut insert_file = tx.prepare(
                "INSERT OR IGNORE INTO findings_files (finding_id, file_path) VALUES (?1, ?2)",
            )?;
            for finding in &report.findings {
                insert_finding.execute(params![
                    report_id,
                    severity_str(finding.severity),
                    finding.weakness_class,
                    serde_json::to_string(finding)?,
                ])?;
                let finding_id = tx.last_insert_rowid();
                for file in files {
                    insert_file.execute(params![finding_id, path_str(file)])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Past findings whose indexed files intersect `files`, newest first.
    /// This is the Kobayashi Maru memory: recurring weakness classes for
    /// in-scope files get attacked first. Ordered newest report first, then
    /// report finding order; each finding appears once even when several of
    /// its files intersect. A malformed stored row is logged and skipped.
    pub fn findings_for_files(&self, files: &[PathBuf]) -> Result<Vec<Finding>, ComputerError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; files.len()].join(", ");
        let sql = format!(
            "SELECT DISTINCT f.id, f.report_id, f.finding_json
             FROM findings f
             JOIN findings_files ff ON ff.finding_id = f.id
             WHERE ff.file_path IN ({placeholders})
             ORDER BY f.report_id DESC, f.id ASC"
        );
        let file_params: Vec<String> = files.iter().map(|p| path_str(p)).collect();
        let conn = self.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(file_params.iter()), |r| {
            r.get::<_, String>(2)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let json = row?;
            match serde_json::from_str::<Finding>(&json) {
                Ok(finding) => out.push(finding),
                Err(err) => tracing::warn!("skipping malformed stored finding: {err}"),
            }
        }
        Ok(out)
    }

    // -- child process registry (orphan cleanup) ---------------------------------

    pub fn register_child_pid(
        &self,
        pid: u32,
        workstream: WorkstreamId,
    ) -> Result<(), ComputerError> {
        self.lock().execute(
            "INSERT INTO child_pids (pid, workstream_id, registered_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(pid) DO UPDATE SET
                 workstream_id = excluded.workstream_id,
                 registered_at_ms = excluded.registered_at_ms",
            params![i64::from(pid), workstream.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn clear_child_pid(&self, pid: u32) -> Result<(), ComputerError> {
        self.lock().execute(
            "DELETE FROM child_pids WHERE pid = ?1",
            params![i64::from(pid)],
        )?;
        Ok(())
    }

    /// Pids recorded as live from a previous run; the app probes and kills
    /// survivors at startup, then clears them. Ascending pid order.
    pub fn recorded_child_pids(&self) -> Result<Vec<u32>, ComputerError> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT pid FROM child_pids ORDER BY pid")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let pid = row?;
            match u32::try_from(pid) {
                Ok(pid) => out.push(pid),
                Err(_) => tracing::warn!("skipping out-of-range recorded pid {pid}"),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ships_computer_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ShipsComputer>();
    }

    #[test]
    fn station_names_round_trip_through_storage_form() {
        for station in Station::ALL {
            assert_eq!(station_from_str(station.as_str()), Some(station));
        }
        assert_eq!(station_from_str("Warp Core"), None);
    }

    #[test]
    fn clamp_u32_saturates_at_bounds() {
        assert_eq!(clamp_u32(-5), 0);
        assert_eq!(clamp_u32(7), 7);
        assert_eq!(clamp_u32(i64::MAX), u32::MAX);
    }
}
