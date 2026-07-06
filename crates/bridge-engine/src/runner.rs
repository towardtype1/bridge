//! Spawning one claude turn: process, stream parsing, timeout, semaphore.

use bridge_compat::{ClaudeInvocation, ResultEvent};
use bridge_core::{BridgeEvent, ClaudeConfig, Station, WorkstreamId};
use crate::ratelimit::RateLimitHit;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Semaphore, broadcast};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("failed to spawn claude: {0}")]
    Spawn(String),
    #[error("engine shutting down")]
    ShuttingDown,
}

/// Context for event attribution and pid registration.
#[derive(Clone)]
pub struct TurnCtx {
    pub workstream: WorkstreamId,
    pub station: Station,
    /// Reserve a Kobayashi slot instead of a general slot.
    pub kobayashi: bool,
    /// Callback target for child pid bookkeeping (orphan cleanup).
    pub pid_register: Option<Arc<dyn Fn(u32, bool) + Send + Sync>>,
}

/// What one invocation produced. `SpawnFailed`/`TimedOut` runs may still
/// carry no result; callers branch on `exit` first.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub exit: ExitClass,
    /// The terminal `result` event, when one was emitted.
    pub result: Option<ResultEvent>,
    /// Set when the stream or result indicated subscription throttling.
    pub rate_limit: Option<RateLimitHit>,
    /// `result.structured_output` convenience copy.
    pub structured_output: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitClass {
    /// Process exited 0.
    Success,
    /// Non-zero exit code.
    NonZero(i32),
    /// Killed by the harness after `turn_timeout_secs`.
    TimedOut,
    SpawnFailed(String),
}

/// Spawns claude children under a two-tier semaphore: `max_concurrent`
/// total slots of which `kobayashi_reserved_slots` only Kobayashi turns
/// may take (they may also take general slots; general turns can never
/// take reserved slots).
pub struct ClaudeRunner {
    _priv: (),
}

impl ClaudeRunner {
    pub fn new(config: ClaudeConfig, events: broadcast::Sender<BridgeEvent>) -> Self {
        todo!()
    }

    /// Expose the general-slot semaphore for Ops introspection.
    pub fn semaphore(&self) -> Arc<Semaphore> {
        todo!()
    }

    /// Run one turn to completion:
    /// 1. acquire the right semaphore slot (kobayashi vs general),
    /// 2. spawn `config.binary_path` with `inv.to_args()`, cwd = inv.cwd,
    ///    `kill_on_drop`, env scrubbed per `bridge_compat::scrubbed_env()`,
    ///    stdin null, stdout/stderr piped,
    /// 3. register the child pid via `ctx.pid_register(pid, true)`,
    /// 4. read stdout line by line through `bridge_compat::parse_line`,
    ///    forwarding AgentOutput / ToolCall / RateLimit / Log events to the
    ///    bus tagged with ctx, collecting the terminal Result event and any
    ///    rate-limit signals (also inspect ApiRetry categories),
    /// 5. enforce `turn_timeout_secs` with tokio::time::timeout; on expiry
    ///    kill the child (SIGTERM, 5s grace, SIGKILL) and return TimedOut,
    /// 6. wait for exit, classify zero/non-zero, capture trailing stderr
    ///    into a Log event when non-zero,
    /// 7. clear the pid via `ctx.pid_register(pid, false)`.
    ///
    /// Never panics on malformed stream lines: they are logged and skipped.
    pub async fn run_turn(&self, inv: ClaudeInvocation, ctx: TurnCtx) -> Result<TurnOutcome, EngineError> {
        todo!()
    }
}
