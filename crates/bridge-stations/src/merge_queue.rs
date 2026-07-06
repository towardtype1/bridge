//! The strictly serialized merge queue.
//!
//! Ordering: plan-graph topology first, completion time second. One entry
//! is processed at a time:
//! 1. rebase the branch onto current main (default) or prepare a merge
//!    commit (config),
//! 2. conflicts -> Helm fix order in that worktree, then re-run checks AND
//!    a fresh Kobayashi round (post-conflict code is new code),
//! 3. GUI confirmation (MergeConfirmationRequested -> ConfirmMerge),
//! 4. local fast-forward/merge by Engineering; never a push,
//! 5. after every merge, all still-active workstreams get rebased onto the
//!    new main head at their next quiet point (between orders, never
//!    mid-run).

use bridge_core::{MergeQueueEntry, WorkstreamId};
use std::collections::HashMap;

/// Pure queue state machine (no I/O): the controller drives transitions
/// and performs the side effects.
#[derive(Debug, Default)]
pub struct MergeQueue {
    _priv: (),
}

impl MergeQueue {
    pub fn new(topo_order: Vec<WorkstreamId>) -> Self {
        todo!()
    }

    /// Workstream finished testing clean; join the queue with the current
    /// completion timestamp (monotonic counter injected by caller).
    pub fn enqueue(&mut self, ws: WorkstreamId, completed_seq: u64, branch: String) {
        todo!()
    }

    /// Next entry to process when the queue head is idle: smallest
    /// (topo_index, completed_seq).
    pub fn next_candidate(&self) -> Option<WorkstreamId> {
        todo!()
    }

    /// Advance an entry's state; invalid transitions panic in debug and
    /// are ignored in release (defensive: the controller owns legality).
    pub fn set_state(&mut self, ws: WorkstreamId, state: bridge_core::MergeQueueState) {
        todo!()
    }

    pub fn remove(&mut self, ws: WorkstreamId) {
        todo!()
    }

    /// Snapshot for the GUI (`MergeQueueUpdate` event), position-ordered.
    pub fn snapshot(&self) -> Vec<MergeQueueEntry> {
        todo!()
    }

    /// True while some entry is between Rebasing and Merging inclusive
    /// (the serialization lock).
    pub fn busy(&self) -> bool {
        todo!()
    }
}
