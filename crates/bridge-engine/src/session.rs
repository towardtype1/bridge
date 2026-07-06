//! Session registry: which claude session belongs to which
//! (workstream, station), and from which directory it must be resumed.
//!
//! Session lookup in the CLI is directory-scoped, so the registry stores
//! the worktree cwd alongside the session id and refuses to hand out a
//! session for a different cwd.

use bridge_core::{SessionId, Station, WorkstreamId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// In-memory registry, hydrated from and persisted to the Ship's Computer
/// by the caller (keeps this crate storage-free).
#[derive(Debug, Default)]
pub struct SessionRegistry {
    inner: Mutex<HashMap<(WorkstreamId, Station), (SessionId, PathBuf)>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, ws: WorkstreamId, station: Station, session: SessionId, cwd: PathBuf) {
        self.inner
            .lock()
            .expect("session registry lock poisoned")
            .insert((ws, station), (session, cwd));
    }

    /// The session to resume for this (workstream, station) IF `cwd`
    /// matches the recorded one; a mismatch returns None (fresh session)
    /// and logs, because resuming from another directory silently fails.
    pub fn resumable(&self, ws: WorkstreamId, station: Station, cwd: &Path) -> Option<SessionId> {
        let guard = self.inner.lock().expect("session registry lock poisoned");
        let (session, recorded_cwd) = guard.get(&(ws, station))?;
        if recorded_cwd == cwd {
            Some(session.clone())
        } else {
            tracing::warn!(
                workstream = %ws,
                station = %station,
                recorded_cwd = %recorded_cwd.display(),
                requested_cwd = %cwd.display(),
                "session cwd mismatch: refusing resume, a fresh session will be started"
            );
            None
        }
    }

    /// Drop all sessions of a workstream (worktree removed / fresh
    /// Kobayashi round wants no memory).
    pub fn forget_workstream(&self, ws: WorkstreamId) {
        self.inner
            .lock()
            .expect("session registry lock poisoned")
            .retain(|(entry_ws, _), _| *entry_ws != ws);
    }

    pub fn all(&self) -> Vec<(WorkstreamId, Station, SessionId, PathBuf)> {
        self.inner
            .lock()
            .expect("session registry lock poisoned")
            .iter()
            .map(|((ws, station), (session, cwd))| (*ws, *station, session.clone(), cwd.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> SessionId {
        SessionId::from(s)
    }

    #[test]
    fn record_then_resumable_with_matching_cwd() {
        let reg = SessionRegistry::new();
        let ws = WorkstreamId::new();
        let cwd = PathBuf::from("/tmp/worktrees/ws-a");
        reg.record(ws, Station::Helm, sid("sess-1"), cwd.clone());
        assert_eq!(reg.resumable(ws, Station::Helm, &cwd), Some(sid("sess-1")));
    }

    #[test]
    fn resumable_with_mismatched_cwd_returns_none() {
        let reg = SessionRegistry::new();
        let ws = WorkstreamId::new();
        reg.record(ws, Station::Helm, sid("sess-1"), PathBuf::from("/tmp/a"));
        assert_eq!(reg.resumable(ws, Station::Helm, Path::new("/tmp/b")), None);
    }

    #[test]
    fn resumable_unknown_key_returns_none() {
        let reg = SessionRegistry::new();
        assert_eq!(
            reg.resumable(WorkstreamId::new(), Station::Science, Path::new("/tmp/a")),
            None
        );
    }

    #[test]
    fn record_overwrites_previous_session_for_same_key() {
        let reg = SessionRegistry::new();
        let ws = WorkstreamId::new();
        let cwd = PathBuf::from("/tmp/a");
        reg.record(ws, Station::Helm, sid("old"), cwd.clone());
        reg.record(ws, Station::Helm, sid("new"), cwd.clone());
        assert_eq!(reg.resumable(ws, Station::Helm, &cwd), Some(sid("new")));
    }

    #[test]
    fn stations_are_tracked_independently() {
        let reg = SessionRegistry::new();
        let ws = WorkstreamId::new();
        let cwd = PathBuf::from("/tmp/a");
        reg.record(ws, Station::Helm, sid("helm"), cwd.clone());
        reg.record(ws, Station::KobayashiMaru, sid("kobayashi"), cwd.clone());
        assert_eq!(reg.resumable(ws, Station::Helm, &cwd), Some(sid("helm")));
        assert_eq!(
            reg.resumable(ws, Station::KobayashiMaru, &cwd),
            Some(sid("kobayashi"))
        );
    }

    #[test]
    fn forget_workstream_drops_all_its_sessions_only() {
        let reg = SessionRegistry::new();
        let ws_a = WorkstreamId::new();
        let ws_b = WorkstreamId::new();
        let cwd = PathBuf::from("/tmp/a");
        reg.record(ws_a, Station::Helm, sid("a-helm"), cwd.clone());
        reg.record(ws_a, Station::KobayashiMaru, sid("a-kob"), cwd.clone());
        reg.record(ws_b, Station::Helm, sid("b-helm"), cwd.clone());
        reg.forget_workstream(ws_a);
        assert_eq!(reg.resumable(ws_a, Station::Helm, &cwd), None);
        assert_eq!(reg.resumable(ws_a, Station::KobayashiMaru, &cwd), None);
        assert_eq!(reg.resumable(ws_b, Station::Helm, &cwd), Some(sid("b-helm")));
    }

    #[test]
    fn all_returns_every_entry() {
        let reg = SessionRegistry::new();
        assert!(reg.all().is_empty());
        let ws = WorkstreamId::new();
        reg.record(ws, Station::Helm, sid("s1"), PathBuf::from("/tmp/a"));
        reg.record(ws, Station::Science, sid("s2"), PathBuf::from("/tmp/a"));
        let mut all = reg.all();
        all.sort_by(|a, b| a.2.0.cmp(&b.2.0));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, ws);
        assert_eq!(all[0].2, sid("s1"));
        assert_eq!(all[1].1, Station::Science);
        assert_eq!(all[1].3, PathBuf::from("/tmp/a"));
    }
}
