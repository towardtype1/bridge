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

use bridge_core::{MergeQueueEntry, MergeQueueState, WorkstreamId};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct QueueEntry {
    branch: String,
    completed_seq: u64,
    state: MergeQueueState,
}

/// Legal state transitions for one queue entry. Everything else is a
/// controller bug: `set_state` panics in debug and ignores in release.
fn transition_is_legal(from: MergeQueueState, to: MergeQueueState) -> bool {
    use MergeQueueState::*;
    matches!(
        (from, to),
        (AwaitingRebase, Rebasing)
            | (Rebasing, ConflictFix)
            | (Rebasing, ChecksRunning)
            | (Rebasing, AwaitingConfirmation)
            | (ConflictFix, Rebasing)
            | (ConflictFix, ChecksRunning)
            | (ChecksRunning, ConflictFix)
            | (ChecksRunning, AwaitingConfirmation)
            | (AwaitingConfirmation, Merging)
            | (Merging, Done)
    )
}

/// Pure queue state machine (no I/O): the controller drives transitions
/// and performs the side effects.
#[derive(Debug, Default)]
pub struct MergeQueue {
    topo: Vec<WorkstreamId>,
    entries: HashMap<WorkstreamId, QueueEntry>,
}

impl MergeQueue {
    pub fn new(topo_order: Vec<WorkstreamId>) -> Self {
        Self {
            topo: topo_order,
            entries: HashMap::new(),
        }
    }

    fn topo_index(&self, ws: WorkstreamId) -> usize {
        // Unknown workstreams (not in the plan topology) sort last.
        self.topo.iter().position(|&t| t == ws).unwrap_or(usize::MAX)
    }

    /// Workstream finished testing clean; join the queue with the current
    /// completion timestamp (monotonic counter injected by caller).
    pub fn enqueue(&mut self, ws: WorkstreamId, completed_seq: u64, branch: String) {
        debug_assert!(
            !self.entries.contains_key(&ws),
            "workstream {ws} enqueued twice"
        );
        self.entries.entry(ws).or_insert(QueueEntry {
            branch,
            completed_seq,
            state: MergeQueueState::AwaitingRebase,
        });
    }

    /// Next entry to process when the queue head is idle: smallest
    /// (topo_index, completed_seq). Returns `None` while an entry is being
    /// processed (the serialization lock).
    pub fn next_candidate(&self) -> Option<WorkstreamId> {
        if self.busy() {
            return None;
        }
        self.entries
            .iter()
            .filter(|(_, e)| e.state == MergeQueueState::AwaitingRebase)
            .min_by_key(|(ws, e)| (self.topo_index(**ws), e.completed_seq))
            .map(|(ws, _)| *ws)
    }

    /// Advance an entry's state; invalid transitions panic in debug and
    /// are ignored in release (defensive: the controller owns legality).
    pub fn set_state(&mut self, ws: WorkstreamId, state: bridge_core::MergeQueueState) {
        let Some(entry) = self.entries.get_mut(&ws) else {
            debug_assert!(false, "set_state for unknown workstream {ws}");
            return;
        };
        if transition_is_legal(entry.state, state) {
            entry.state = state;
        } else {
            debug_assert!(
                false,
                "illegal merge queue transition {:?} -> {state:?} for {ws}",
                entry.state
            );
        }
    }

    pub fn remove(&mut self, ws: WorkstreamId) {
        self.entries.remove(&ws);
    }

    /// Current state of an enqueued workstream, if present.
    pub fn state_of(&self, ws: WorkstreamId) -> Option<MergeQueueState> {
        self.entries.get(&ws).map(|e| e.state)
    }

    pub fn contains(&self, ws: WorkstreamId) -> bool {
        self.entries.contains_key(&ws)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Snapshot for the GUI (`MergeQueueUpdate` event), position-ordered.
    pub fn snapshot(&self) -> Vec<MergeQueueEntry> {
        let mut rows: Vec<(usize, u64, WorkstreamId, &QueueEntry)> = self
            .entries
            .iter()
            .map(|(&ws, e)| (self.topo_index(ws), e.completed_seq, ws, e))
            .collect();
        rows.sort_by_key(|(topo, seq, _, _)| (*topo, *seq));
        rows.into_iter()
            .enumerate()
            .map(|(i, (_, _, ws, e))| MergeQueueEntry {
                workstream: ws,
                branch: e.branch.clone(),
                position: i as u32,
                state: e.state,
            })
            .collect()
    }

    /// True while some entry is between Rebasing and Merging inclusive
    /// (the serialization lock).
    pub fn busy(&self) -> bool {
        use MergeQueueState::*;
        self.entries
            .values()
            .any(|e| matches!(e.state, Rebasing | ConflictFix | ChecksRunning | AwaitingConfirmation | Merging))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MergeQueueState::*;

    fn ids(n: usize) -> Vec<WorkstreamId> {
        (0..n).map(|_| WorkstreamId::new()).collect()
    }

    #[test]
    fn candidate_orders_by_topology_before_completion_time() {
        let topo = ids(3);
        let mut q = MergeQueue::new(topo.clone());
        // The topologically-later entry completed first.
        q.enqueue(topo[2], 1, "b-c".into());
        q.enqueue(topo[0], 2, "b-a".into());
        assert_eq!(q.next_candidate(), Some(topo[0]));
    }

    #[test]
    fn candidate_breaks_topo_ties_by_completion_seq() {
        // Workstreams missing from the topo order share topo_index MAX;
        // completion order decides.
        let mut q = MergeQueue::new(vec![]);
        let (a, b) = (WorkstreamId::new(), WorkstreamId::new());
        q.enqueue(b, 7, "b".into());
        q.enqueue(a, 9, "a".into());
        assert_eq!(q.next_candidate(), Some(b));
    }

    #[test]
    fn unknown_workstreams_sort_after_planned_ones() {
        let topo = ids(1);
        let mut q = MergeQueue::new(topo.clone());
        let stranger = WorkstreamId::new();
        q.enqueue(stranger, 1, "s".into());
        q.enqueue(topo[0], 99, "p".into());
        assert_eq!(q.next_candidate(), Some(topo[0]));
    }

    #[test]
    fn busy_locks_the_queue_until_done() {
        let topo = ids(2);
        let mut q = MergeQueue::new(topo.clone());
        q.enqueue(topo[0], 1, "a".into());
        q.enqueue(topo[1], 2, "b".into());
        assert!(!q.busy());
        assert_eq!(q.next_candidate(), Some(topo[0]));

        q.set_state(topo[0], Rebasing);
        assert!(q.busy());
        assert_eq!(q.next_candidate(), None, "busy queue yields no candidate");

        for s in [ChecksRunning, AwaitingConfirmation, Merging] {
            q.set_state(topo[0], s);
            assert!(q.busy(), "{s:?} must hold the lock");
        }
        q.set_state(topo[0], Done);
        assert!(!q.busy());
        assert_eq!(q.next_candidate(), Some(topo[1]));
    }

    #[test]
    fn conflict_fix_holds_the_lock() {
        let topo = ids(2);
        let mut q = MergeQueue::new(topo.clone());
        q.enqueue(topo[0], 1, "a".into());
        q.enqueue(topo[1], 2, "b".into());
        q.set_state(topo[0], Rebasing);
        q.set_state(topo[0], ConflictFix);
        assert!(q.busy());
        assert_eq!(q.next_candidate(), None);
        // conflict fix -> checks -> confirm -> merge -> done
        q.set_state(topo[0], ChecksRunning);
        q.set_state(topo[0], AwaitingConfirmation);
        q.set_state(topo[0], Merging);
        q.set_state(topo[0], Done);
        assert_eq!(q.next_candidate(), Some(topo[1]));
    }

    #[test]
    fn snapshot_is_position_ordered_and_complete() {
        let topo = ids(3);
        let mut q = MergeQueue::new(topo.clone());
        q.enqueue(topo[1], 1, "b-b".into());
        q.enqueue(topo[0], 2, "b-a".into());
        q.enqueue(topo[2], 3, "b-c".into());
        let snap = q.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(
            snap.iter().map(|e| e.workstream).collect::<Vec<_>>(),
            vec![topo[0], topo[1], topo[2]]
        );
        assert_eq!(snap.iter().map(|e| e.position).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(snap[0].branch, "b-a");
        assert!(snap.iter().all(|e| e.state == AwaitingRebase));
    }

    #[test]
    fn remove_drops_the_entry() {
        let topo = ids(2);
        let mut q = MergeQueue::new(topo.clone());
        q.enqueue(topo[0], 1, "a".into());
        q.enqueue(topo[1], 2, "b".into());
        q.remove(topo[0]);
        assert!(!q.contains(topo[0]));
        assert_eq!(q.snapshot().len(), 1);
        assert_eq!(q.next_candidate(), Some(topo[1]));
        q.remove(topo[1]);
        assert!(q.is_empty());
        assert_eq!(q.next_candidate(), None);
    }

    #[test]
    fn state_of_reports_current_state() {
        let topo = ids(1);
        let mut q = MergeQueue::new(topo.clone());
        assert_eq!(q.state_of(topo[0]), None);
        q.enqueue(topo[0], 1, "a".into());
        assert_eq!(q.state_of(topo[0]), Some(AwaitingRebase));
        q.set_state(topo[0], Rebasing);
        assert_eq!(q.state_of(topo[0]), Some(Rebasing));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "illegal merge queue transition")]
    fn invalid_transition_panics_in_debug() {
        let topo = ids(1);
        let mut q = MergeQueue::new(topo.clone());
        q.enqueue(topo[0], 1, "a".into());
        q.set_state(topo[0], Done); // AwaitingRebase -> Done is illegal
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "unknown workstream")]
    fn set_state_for_unknown_workstream_panics_in_debug() {
        let mut q = MergeQueue::new(vec![]);
        q.set_state(WorkstreamId::new(), Rebasing);
    }

    /// The release behavior ("ignored") is encoded in the legality table;
    /// exercise the table directly since tests build with debug assertions.
    #[test]
    fn transition_table_rejects_regressions_and_skips() {
        assert!(transition_is_legal(AwaitingRebase, Rebasing));
        assert!(transition_is_legal(Rebasing, AwaitingConfirmation));
        assert!(transition_is_legal(Merging, Done));
        for (from, to) in [
            (AwaitingRebase, Done),
            (AwaitingRebase, Merging),
            (Done, Merging),
            (Merging, Rebasing),
            (AwaitingConfirmation, AwaitingRebase),
            (Rebasing, Rebasing),
        ] {
            assert!(!transition_is_legal(from, to), "{from:?} -> {to:?} must be illegal");
        }
    }
}
