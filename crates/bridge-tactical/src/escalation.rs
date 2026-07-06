//! Escalation broker: holds a hook call open until the user decides.

use bridge_core::{EscalationId, EscalationTicket, UserDecision};
use std::time::Duration;
use tokio::sync::oneshot;

/// Pending escalations keyed by id. `open` returns a receiver the server
/// awaits (with timeout); `resolve` is called from the GUI command path.
/// Timeout or a dropped ticket resolves as deny (fail closed).
pub struct EscalationBroker {
    _priv: (),
}

impl EscalationBroker {
    pub fn new() -> Self {
        todo!()
    }

    /// Register a ticket and get the decision receiver. The broker emits
    /// the ticket on the event bus separately (server layer's job).
    pub fn open(&self, ticket: EscalationTicket) -> oneshot::Receiver<UserDecision> {
        todo!()
    }

    /// Resolve from the GUI. Unknown/expired ids are ignored (the hook
    /// already failed closed).
    pub fn resolve(&self, id: EscalationId, decision: UserDecision) {
        todo!()
    }

    /// Await a decision with a deadline; None (timeout / dropped sender)
    /// must be treated as deny by the caller.
    pub async fn await_decision(
        rx: oneshot::Receiver<UserDecision>,
        timeout: Duration,
    ) -> Option<UserDecision> {
        todo!()
    }

    /// Ids currently pending (GUI reconciliation after restart).
    pub fn pending(&self) -> Vec<EscalationId> {
        todo!()
    }
}
