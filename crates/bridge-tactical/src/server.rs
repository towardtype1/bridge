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
use crate::policy::{PolicyEngine, PolicyVerdict};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bridge_compat::{
    HookPayload, hook_response_allow, hook_response_deny, hook_response_passthrough,
};
use bridge_core::wire::HookWireRequest;
use bridge_core::{
    BridgeEvent, DecisionKind, DecisionSource, EscalationId, EscalationTicket, HookDecisionRecord,
    UserDecision, WorkstreamId,
};
use chrono::Utc;
use rand::RngCore;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// One-shot deep scan hook: given the hook payload JSON, return true when
/// the call should be allowed to pass. Implemented by the engine crate
/// (a cheap `claude -p` review); injected to keep tactical engine-free.
pub type DeepScanFn =
    Arc<dyn Fn(serde_json::Value) -> tokio::sync::oneshot::Receiver<bool> + Send + Sync>;

struct ServerState {
    policy: Arc<PolicyEngine>,
    broker: Arc<EscalationBroker>,
    events: broadcast::Sender<BridgeEvent>,
    escalation_timeout: Duration,
    deep_scan: Option<DeepScanFn>,
    tokens: RwLock<HashMap<WorkstreamId, String>>,
}

pub struct ControlServer {
    state: Arc<ServerState>,
}

/// Handle to the bound server: address, token registry, shutdown.
pub struct ControlServerHandle {
    addr: SocketAddr,
    state: Arc<ServerState>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl ControlServer {
    pub fn new(
        policy: Arc<PolicyEngine>,
        broker: Arc<EscalationBroker>,
        events: broadcast::Sender<BridgeEvent>,
        escalation_timeout_secs: u64,
        deep_scan: Option<DeepScanFn>,
    ) -> Self {
        Self {
            state: Arc::new(ServerState {
                policy,
                broker,
                events,
                escalation_timeout: Duration::from_secs(escalation_timeout_secs),
                deep_scan,
                tokens: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Bind 127.0.0.1 on an ephemeral port and start serving. The handle
    /// exposes the resolved base URL for settings rendering.
    pub async fn bind(self) -> std::io::Result<ControlServerHandle> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let router = Router::new()
            .route("/healthz", get(healthz))
            .route("/hook", post(hook))
            .with_state(self.state.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let served = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
            if let Err(err) = served {
                tracing::error!("control server exited with error: {err}");
            }
        });
        Ok(ControlServerHandle {
            addr,
            state: self.state,
            shutdown_tx,
            task,
        })
    }
}

impl ControlServerHandle {
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Issue (and register) a fresh random token for a workstream.
    /// Returns the token string to embed in that worktree's settings.
    pub fn issue_token(&self, ws: WorkstreamId) -> String {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = bytes.iter().fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        });
        self.state
            .tokens
            .write()
            .expect("token registry poisoned")
            .insert(ws, token.clone());
        token
    }

    pub fn revoke_token(&self, ws: WorkstreamId) {
        self.state
            .tokens
            .write()
            .expect("token registry poisoned")
            .remove(&ws);
    }

    /// Graceful shutdown of the listener.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.task.await;
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn hook(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(req): Json<HookWireRequest>,
) -> Response {
    let started = Instant::now();
    if !authorized(&state, &headers, req.workstream_id) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let payload = match HookPayload::parse(&req.payload) {
        Ok(payload) => payload,
        Err(err) => {
            // Fail closed: an unparseable payload denies with the most
            // restrictive (PreToolUse) response shape.
            let reason = format!("malformed hook payload: {err}");
            emit_decision(
                &state,
                HookDecisionRecord {
                    timestamp: Utc::now(),
                    workstream: req.workstream_id,
                    hook_event: "unknown".into(),
                    tool_name: None,
                    decision: DecisionKind::Deny,
                    reason: Some(reason.clone()),
                    rule: "prime.malformed_payload".into(),
                    source: DecisionSource::PrimeDirective,
                    latency_ms: started.elapsed().as_millis() as u64,
                },
            );
            return (
                StatusCode::OK,
                Json(hook_response_deny("PreToolUse", &reason)),
            )
                .into_response();
        }
    };

    let verdict = state.policy.decide(req.workstream_id, &payload);
    let outcome = adjudicate(&state, req.workstream_id, &payload, &req.payload, verdict).await;
    emit_decision(
        &state,
        HookDecisionRecord {
            timestamp: Utc::now(),
            workstream: req.workstream_id,
            hook_event: payload.hook_event_name.clone(),
            tool_name: payload.tool_name.clone(),
            decision: outcome.decision,
            reason: outcome.reason,
            rule: outcome.rule,
            source: outcome.source,
            latency_ms: started.elapsed().as_millis() as u64,
        },
    );
    (StatusCode::OK, Json(outcome.body)).into_response()
}

struct Adjudication {
    body: serde_json::Value,
    decision: DecisionKind,
    source: DecisionSource,
    rule: String,
    reason: Option<String>,
}

async fn adjudicate(
    state: &ServerState,
    ws: WorkstreamId,
    payload: &HookPayload,
    raw_payload: &serde_json::Value,
    verdict: PolicyVerdict,
) -> Adjudication {
    let event = payload.hook_event_name.as_str();
    match verdict {
        PolicyVerdict::Pass { suspicious } => {
            if suspicious && let Some(scan) = &state.deep_scan {
                match scan(raw_payload.clone()).await {
                    Ok(true) => Adjudication {
                        body: hook_response_passthrough(),
                        decision: DecisionKind::Allow,
                        source: DecisionSource::DeepScan,
                        rule: "deep_scan".into(),
                        reason: None,
                    },
                    // false, or a dropped scanner: fail closed.
                    _ => {
                        let reason = "deep scan rejected the call (fail closed)".to_string();
                        Adjudication {
                            body: hook_response_deny(event, &reason),
                            decision: DecisionKind::Deny,
                            source: DecisionSource::DeepScan,
                            rule: "deep_scan".into(),
                            reason: Some(reason),
                        }
                    }
                }
            } else {
                Adjudication {
                    body: hook_response_passthrough(),
                    decision: DecisionKind::Allow,
                    source: DecisionSource::Passthrough,
                    rule: "passthrough".into(),
                    reason: None,
                }
            }
        }
        PolicyVerdict::Deny { reason, rule } => Adjudication {
            body: hook_response_deny(event, &reason),
            decision: DecisionKind::Deny,
            source: source_for_rule(&rule),
            rule,
            reason: Some(reason),
        },
        PolicyVerdict::Escalate { question, rule } => {
            escalate(state, ws, payload, question, rule).await
        }
    }
}

async fn escalate(
    state: &ServerState,
    ws: WorkstreamId,
    payload: &HookPayload,
    question: String,
    rule: String,
) -> Adjudication {
    let event = payload.hook_event_name.as_str();
    let ticket = EscalationTicket {
        id: EscalationId::new(),
        workstream: ws,
        question: question.clone(),
        tool_name: payload.tool_name.clone(),
        tool_input_summary: summarize_input(payload.tool_input.as_ref()),
        requested_at: Utc::now(),
        expires_at: Utc::now()
            + chrono::Duration::from_std(state.escalation_timeout)
                .unwrap_or_else(|_| chrono::Duration::seconds(0)),
    };
    let rx = state.broker.open(ticket.clone());
    let _ = state
        .events
        .send(BridgeEvent::EscalationRequested(ticket.clone()));

    match EscalationBroker::await_decision(rx, state.escalation_timeout).await {
        Some(UserDecision::Approve) => {
            let _ = state.events.send(BridgeEvent::EscalationResolved {
                id: ticket.id,
                decision: UserDecision::Approve,
            });
            let reason = format!("user approved escalation: {question}");
            Adjudication {
                body: hook_response_allow(event, &reason),
                decision: DecisionKind::Allow,
                source: DecisionSource::User,
                rule,
                reason: Some(reason),
            }
        }
        Some(UserDecision::Deny { reason }) => {
            let _ = state.events.send(BridgeEvent::EscalationResolved {
                id: ticket.id,
                decision: UserDecision::Deny {
                    reason: reason.clone(),
                },
            });
            Adjudication {
                body: hook_response_deny(event, &reason),
                decision: DecisionKind::Deny,
                source: DecisionSource::User,
                rule,
                reason: Some(reason),
            }
        }
        None => {
            let reason = "escalation timed out (fail closed to deny)".to_string();
            // Remove the stale entry; a late user resolve is then a no-op.
            state.broker.resolve(
                ticket.id,
                UserDecision::Deny {
                    reason: reason.clone(),
                },
            );
            let _ = state.events.send(BridgeEvent::EscalationResolved {
                id: ticket.id,
                decision: UserDecision::Deny {
                    reason: reason.clone(),
                },
            });
            Adjudication {
                body: hook_response_deny(event, &reason),
                decision: DecisionKind::Deny,
                source: source_for_rule(&rule),
                rule,
                reason: Some(reason),
            }
        }
    }
}

fn authorized(state: &ServerState, headers: &HeaderMap, ws: WorkstreamId) -> bool {
    let Some(presented) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let tokens = state.tokens.read().expect("token registry poisoned");
    tokens
        .get(&ws)
        .is_some_and(|expected| constant_time_eq(expected.as_bytes(), presented.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The policy engine names rules with `prime.` / `config.` / `red_alert`
/// prefixes; map them onto the event bus decision source.
fn source_for_rule(rule: &str) -> DecisionSource {
    if rule.starts_with("prime") {
        DecisionSource::PrimeDirective
    } else if rule.starts_with("config") {
        DecisionSource::ConfigRule
    } else if rule.starts_with("red_alert") {
        DecisionSource::RedAlert
    } else if rule.starts_with("deep_scan") {
        DecisionSource::DeepScan
    } else {
        DecisionSource::Passthrough
    }
}

fn summarize_input(input: Option<&serde_json::Value>) -> String {
    const MAX: usize = 200;
    let rendered = input
        .map(|v| v.to_string())
        .unwrap_or_else(|| "{}".to_string());
    if rendered.chars().count() <= MAX {
        rendered
    } else {
        let truncated: String = rendered.chars().take(MAX).collect();
        format!("{truncated}...")
    }
}

fn emit_decision(state: &ServerState, record: HookDecisionRecord) {
    // No receivers is fine (e.g. GUI not yet attached); the send result
    // is deliberately ignored.
    let _ = state.events.send(BridgeEvent::HookDecision(record));
}
