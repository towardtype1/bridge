//! Escalation broker: holds a hook call open until the user decides.

use bridge_core::{EscalationId, EscalationTicket, UserDecision};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// Pending escalations keyed by id. `open` returns a receiver the server
/// awaits (with timeout); `resolve` is called from the GUI command path.
/// Timeout or a dropped ticket resolves as deny (fail closed).
pub struct EscalationBroker {
    pending: Mutex<HashMap<EscalationId, oneshot::Sender<UserDecision>>>,
}

impl EscalationBroker {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register a ticket and get the decision receiver. The broker emits
    /// the ticket on the event bus separately (server layer's job).
    ///
    /// Re-opening an id replaces the previous sender; its receiver then
    /// resolves to None (deny), which keeps the fail-closed invariant.
    pub fn open(&self, ticket: EscalationTicket) -> oneshot::Receiver<UserDecision> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("escalation registry poisoned")
            .insert(ticket.id, tx);
        rx
    }

    /// Resolve from the GUI. Unknown/expired ids are ignored (the hook
    /// already failed closed). Always removes the id from the pending set,
    /// so the server also calls this on timeout to clean up.
    pub fn resolve(&self, id: EscalationId, decision: UserDecision) {
        let sender = self
            .pending
            .lock()
            .expect("escalation registry poisoned")
            .remove(&id);
        if let Some(tx) = sender {
            // A dropped receiver (timed-out hook) makes this send fail;
            // that is fine, the hook already resolved to deny.
            let _ = tx.send(decision);
        }
    }

    /// Await a decision with a deadline; None (timeout / dropped sender)
    /// must be treated as deny by the caller.
    pub async fn await_decision(
        rx: oneshot::Receiver<UserDecision>,
        timeout: Duration,
    ) -> Option<UserDecision> {
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(decision)) => Some(decision),
            // Elapsed or sender dropped: fail closed upstream.
            _ => None,
        }
    }

    /// Ids currently pending (GUI reconciliation after restart).
    pub fn pending(&self) -> Vec<EscalationId> {
        self.pending
            .lock()
            .expect("escalation registry poisoned")
            .keys()
            .copied()
            .collect()
    }
}

impl Default for EscalationBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::WorkstreamId;
    use chrono::Utc;

    fn ticket() -> EscalationTicket {
        EscalationTicket {
            id: EscalationId::new(),
            workstream: WorkstreamId::new(),
            question: "Allow `git push origin x`?".into(),
            tool_name: Some("Bash".into()),
            tool_input_summary: "git push origin x".into(),
            requested_at: Utc::now(),
            expires_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn open_then_resolve_delivers_decision() {
        let broker = EscalationBroker::new();
        let t = ticket();
        let id = t.id;
        let rx = broker.open(t);
        broker.resolve(id, UserDecision::Approve);
        let decision = EscalationBroker::await_decision(rx, Duration::from_secs(5)).await;
        assert_eq!(decision, Some(UserDecision::Approve));
    }

    #[tokio::test]
    async fn deny_decision_round_trips() {
        let broker = EscalationBroker::new();
        let t = ticket();
        let id = t.id;
        let rx = broker.open(t);
        broker.resolve(
            id,
            UserDecision::Deny {
                reason: "not on my bridge".into(),
            },
        );
        let decision = EscalationBroker::await_decision(rx, Duration::from_secs(5)).await;
        assert_eq!(
            decision,
            Some(UserDecision::Deny {
                reason: "not on my bridge".into()
            })
        );
    }

    #[tokio::test]
    async fn timeout_returns_none() {
        let broker = EscalationBroker::new();
        let rx = broker.open(ticket());
        let decision = EscalationBroker::await_decision(rx, Duration::from_millis(20)).await;
        assert_eq!(decision, None);
    }

    #[tokio::test]
    async fn resolve_after_timeout_is_ignored_gracefully() {
        let broker = EscalationBroker::new();
        let t = ticket();
        let id = t.id;
        let rx = broker.open(t);
        let decision = EscalationBroker::await_decision(rx, Duration::from_millis(20)).await;
        assert_eq!(decision, None);
        // The receiver is gone; this must not panic and must clean up.
        broker.resolve(id, UserDecision::Approve);
        assert!(broker.pending().is_empty());
    }

    #[tokio::test]
    async fn resolve_unknown_id_is_ignored() {
        let broker = EscalationBroker::new();
        broker.resolve(EscalationId::new(), UserDecision::Approve);
        assert!(broker.pending().is_empty());
    }

    #[tokio::test]
    async fn pending_lists_open_tickets_until_resolved() {
        let broker = EscalationBroker::new();
        let a = ticket();
        let b = ticket();
        let (id_a, id_b) = (a.id, b.id);
        let _rx_a = broker.open(a);
        let _rx_b = broker.open(b);
        let mut pending = broker.pending();
        pending.sort();
        let mut expected = vec![id_a, id_b];
        expected.sort();
        assert_eq!(pending, expected);
        broker.resolve(id_a, UserDecision::Approve);
        assert_eq!(broker.pending(), vec![id_b]);
    }

    #[tokio::test]
    async fn dropped_broker_resolves_receiver_to_none() {
        let broker = EscalationBroker::new();
        let rx = broker.open(ticket());
        drop(broker);
        let decision = EscalationBroker::await_decision(rx, Duration::from_secs(5)).await;
        assert_eq!(decision, None);
    }
}
