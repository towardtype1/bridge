//! Session registry: which claude session belongs to which
//! (workstream, station), and from which directory it must be resumed.
//!
//! Session lookup in the CLI is directory-scoped, so the registry stores
//! the worktree cwd alongside the session id and refuses to hand out a
//! session for a different cwd.

use bridge_core::{SessionId, Station, WorkstreamId};
use std::path::{Path, PathBuf};

/// In-memory registry, hydrated from and persisted to the Ship's Computer
/// by the caller (keeps this crate storage-free).
#[derive(Debug, Default)]
pub struct SessionRegistry {
    _priv: (),
}

impl SessionRegistry {
    pub fn new() -> Self {
        todo!()
    }

    pub fn record(&self, ws: WorkstreamId, station: Station, session: SessionId, cwd: PathBuf) {
        todo!()
    }

    /// The session to resume for this (workstream, station) IF `cwd`
    /// matches the recorded one; a mismatch returns None (fresh session)
    /// and logs, because resuming from another directory silently fails.
    pub fn resumable(&self, ws: WorkstreamId, station: Station, cwd: &Path) -> Option<SessionId> {
        todo!()
    }

    /// Drop all sessions of a workstream (worktree removed / fresh
    /// Kobayashi round wants no memory).
    pub fn forget_workstream(&self, ws: WorkstreamId) {
        todo!()
    }

    pub fn all(&self) -> Vec<(WorkstreamId, Station, SessionId, PathBuf)> {
        todo!()
    }
}
