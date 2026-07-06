//! The control server: axum on 127.0.0.1, ephemeral port.
//!
//! Routes:
//! - `POST /hook`  body: `bridge_core::wire::HookWireRequest`,
//!   auth: `Authorization: Bearer <per-workstream token>`.
//!   Response: 200 with the raw hook response JSON the helper prints
//!   verbatim. Invalid token or unknown workstream: 401 (helper fails
//!   closed to deny).
//! - `GET /healthz` -> 200 "ok".
//!
//! For each adjudication the server:
//! 1. verifies the bearer token against the workstream registry,
//! 2. parses the payload via `bridge_compat::HookPayload::parse`,
//! 3. asks the `PolicyEngine`,
//! 4. maps the verdict: Pass -> passthrough `{}` (plus optional deep scan
//!    first when the verdict is suspicious and deep_scan is enabled),
//!    Deny -> `hook_response_deny`, Escalate -> ticket via the broker,
//!    await user decision with `escalation_timeout_secs` deadline,
//!    Approve -> `hook_response_allow`, Deny/timeout -> deny,
//! 5. emits a `HookDecision` BridgeEvent with latency, and a
//!    `EscalationRequested`/`EscalationResolved` pair around escalations.

use crate::escalation::EscalationBroker;
use crate::policy::PolicyEngine;
use bridge_core::{BridgeEvent, WorkstreamId};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;

/// One-shot deep scan hook: given the hook payload JSON, return true when
/// the call should be allowed to pass. Implemented by the engine crate
/// (a cheap `claude -p` review); injected to keep tactical engine-free.
pub type DeepScanFn = Arc<
    dyn Fn(serde_json::Value) -> tokio::sync::oneshot::Receiver<bool> + Send + Sync,
>;

pub struct ControlServer {
    _priv: (),
}

/// Handle to the bound server: address, token registry, shutdown.
pub struct ControlServerHandle {
    _priv: (),
}

impl ControlServer {
    pub fn new(
        policy: Arc<PolicyEngine>,
        broker: Arc<EscalationBroker>,
        events: broadcast::Sender<BridgeEvent>,
        escalation_timeout_secs: u64,
        deep_scan: Option<DeepScanFn>,
    ) -> Self {
        todo!()
    }

    /// Bind 127.0.0.1 on an ephemeral port and start serving. The handle
    /// exposes the resolved base URL for settings rendering.
    pub async fn bind(self) -> std::io::Result<ControlServerHandle> {
        todo!()
    }
}

impl ControlServerHandle {
    pub fn base_url(&self) -> String {
        todo!()
    }

    pub fn addr(&self) -> SocketAddr {
        todo!()
    }

    /// Issue (and register) a fresh random token for a workstream.
    /// Returns the token string to embed in that worktree's settings.
    pub fn issue_token(&self, ws: WorkstreamId) -> String {
        todo!()
    }

    pub fn revoke_token(&self, ws: WorkstreamId) {
        todo!()
    }

    /// Graceful shutdown of the listener.
    pub async fn shutdown(self) {
        todo!()
    }
}
