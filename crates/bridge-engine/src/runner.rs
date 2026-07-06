//! Spawning one claude turn: process, stream parsing, timeout, semaphore.

use bridge_compat::{
    ApiErrorCategory, AssistantContent, ClaudeInvocation, ResultEvent, StreamEvent, parse_line,
    scrubbed_env,
};
use bridge_core::{
    BridgeEvent, ClaudeConfig, LogEntry, LogLevel, RateLimitState, Station, WorkstreamId,
};
use crate::ratelimit::RateLimitHit;
use chrono::DateTime;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

/// SIGTERM-to-SIGKILL grace period when a turn times out.
const KILL_GRACE: Duration = Duration::from_secs(5);
/// How long to wait for trailing stderr after the child exited.
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest tool-input summary forwarded in a `ToolCall` event.
const TOOL_SUMMARY_MAX_CHARS: usize = 160;

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

/// Everything gathered from the child's stdout stream during one turn.
#[derive(Default)]
struct StreamState {
    result: Option<ResultEvent>,
    rate_limit: Option<RateLimitHit>,
    /// An api_retry with a throttling category was seen; only meaningful
    /// when the turn subsequently fails.
    api_retry_throttled: bool,
}

/// Spawns claude children under a two-tier semaphore: `max_concurrent`
/// total slots of which `kobayashi_reserved_slots` only Kobayashi turns
/// may take (they may also take general slots; general turns can never
/// take reserved slots).
pub struct ClaudeRunner {
    config: ClaudeConfig,
    events: broadcast::Sender<BridgeEvent>,
    general: Arc<Semaphore>,
    reserved: Arc<Semaphore>,
}

impl ClaudeRunner {
    pub fn new(config: ClaudeConfig, events: broadcast::Sender<BridgeEvent>) -> Self {
        // Config validation guarantees reserved < max_concurrent; the
        // clamps below keep an unvalidated config from deadlocking.
        let reserved_slots = config
            .kobayashi_reserved_slots
            .min(config.max_concurrent.saturating_sub(1)) as usize;
        let general_slots = (config.max_concurrent as usize)
            .saturating_sub(reserved_slots)
            .max(1);
        Self {
            config,
            events,
            general: Arc::new(Semaphore::new(general_slots)),
            reserved: Arc::new(Semaphore::new(reserved_slots)),
        }
    }

    /// Expose the general-slot semaphore for Ops introspection.
    pub fn semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.general)
    }

    /// Acquire a slot. Kobayashi turns prefer their reserved slot and fall
    /// back to racing for whichever of (reserved, general) frees first;
    /// general turns only ever wait on the general semaphore.
    async fn acquire_slot(&self, kobayashi: bool) -> Result<OwnedSemaphorePermit, EngineError> {
        if kobayashi && self.reserved.available_permits() > 0
            && let Ok(permit) = Arc::clone(&self.reserved).try_acquire_owned()
        {
            return Ok(permit);
        }
        if kobayashi {
            tokio::select! {
                permit = Arc::clone(&self.reserved).acquire_owned() => {
                    permit.map_err(|_| EngineError::ShuttingDown)
                }
                permit = Arc::clone(&self.general).acquire_owned() => {
                    permit.map_err(|_| EngineError::ShuttingDown)
                }
            }
        } else {
            Arc::clone(&self.general)
                .acquire_owned()
                .await
                .map_err(|_| EngineError::ShuttingDown)
        }
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
        let _permit = self.acquire_slot(ctx.kobayashi).await?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.args(inv.to_args())
            .current_dir(&inv.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for var in scrubbed_env() {
            cmd.env_remove(var);
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                let msg = format!("{}: {err}", self.config.binary_path);
                self.emit_log(&ctx, LogLevel::Error, format!("failed to spawn claude: {msg}"));
                return Ok(TurnOutcome {
                    exit: ExitClass::SpawnFailed(msg),
                    result: None,
                    rate_limit: None,
                    structured_output: None,
                });
            }
        };
        let pid = child.id();
        if let (Some(register), Some(pid)) = (ctx.pid_register.as_ref(), pid) {
            register(pid, true);
        }

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut buf).await;
            buf
        });

        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.turn_timeout_secs.max(1));
        let mut state = StreamState::default();
        let reached_eof = tokio::time::timeout_at(deadline, async {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => self.handle_line(&line, &ctx, &mut state),
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!(error = %err, "error reading claude stdout; stopping stream");
                        break;
                    }
                }
            }
        })
        .await
        .is_ok();

        let exit = if reached_eof {
            match tokio::time::timeout_at(deadline, child.wait()).await {
                Ok(Ok(status)) => {
                    if status.success() {
                        ExitClass::Success
                    } else {
                        ExitClass::NonZero(status.code().unwrap_or(-1))
                    }
                }
                Ok(Err(err)) => ExitClass::SpawnFailed(format!("wait on claude child failed: {err}")),
                Err(_) => {
                    self.kill_gracefully(&mut child, pid).await;
                    ExitClass::TimedOut
                }
            }
        } else {
            self.kill_gracefully(&mut child, pid).await;
            ExitClass::TimedOut
        };

        let stderr_text = match tokio::time::timeout(STDERR_DRAIN_TIMEOUT, stderr_task).await {
            Ok(Ok(text)) => text,
            _ => String::new(),
        };
        match &exit {
            ExitClass::NonZero(code) => {
                let mut msg = format!("claude exited with code {code}");
                if !stderr_text.trim().is_empty() {
                    msg.push_str(": ");
                    msg.push_str(stderr_text.trim());
                }
                self.emit_log(&ctx, LogLevel::Error, msg);
            }
            ExitClass::TimedOut => {
                self.emit_log(
                    &ctx,
                    LogLevel::Warn,
                    format!(
                        "claude turn timed out after {}s and was killed",
                        self.config.turn_timeout_secs
                    ),
                );
            }
            ExitClass::Success | ExitClass::SpawnFailed(_) => {}
        }

        Self::finalize_rate_limit(&mut state, &exit);

        if let (Some(register), Some(pid)) = (ctx.pid_register.as_ref(), pid) {
            register(pid, false);
        }

        let structured_output = state
            .result
            .as_ref()
            .and_then(|res| res.structured_output.clone());
        Ok(TurnOutcome {
            exit,
            result: state.result,
            rate_limit: state.rate_limit,
            structured_output,
        })
    }

    /// SIGTERM, wait up to [`KILL_GRACE`], then SIGKILL. Always reaps.
    async fn kill_gracefully(&self, child: &mut Child, pid: Option<u32>) {
        if let Some(pid) = pid {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
            if tokio::time::timeout(KILL_GRACE, child.wait()).await.is_ok() {
                return;
            }
            tracing::warn!(pid, "claude child ignored SIGTERM; sending SIGKILL");
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    /// Post-stream reconciliation of throttling signals that only become
    /// meaningful once the exit class is known.
    fn finalize_rate_limit(state: &mut StreamState, exit: &ExitClass) {
        if state.rate_limit.is_some() {
            return;
        }
        if let Some(res) = &state.result
            && res.looks_rate_limited()
        {
            state.rate_limit = Some(RateLimitHit {
                retry_at: None,
                trigger: format!("result looks rate limited (subtype \"{}\")", res.subtype),
            });
            return;
        }
        let turn_failed = !matches!(exit, ExitClass::Success)
            || state.result.as_ref().is_some_and(|res| res.is_error);
        if state.api_retry_throttled && turn_failed {
            state.rate_limit = Some(RateLimitHit {
                retry_at: None,
                trigger: "api_retry throttling preceded a failed turn".into(),
            });
        }
    }

    fn handle_line(&self, line: &str, ctx: &TurnCtx, state: &mut StreamState) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        match parse_line(trimmed) {
            Ok(event) => self.handle_event(event, ctx, state),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    line = %truncate(trimmed, 200),
                    "skipping malformed stream line"
                );
                self.emit_log(
                    ctx,
                    LogLevel::Warn,
                    format!("skipping malformed stream line: {err}"),
                );
            }
        }
    }

    fn handle_event(&self, event: StreamEvent, ctx: &TurnCtx, state: &mut StreamState) {
        match event {
            StreamEvent::SystemInit(init) => {
                tracing::debug!(
                    session_id = %init.session_id,
                    model = %init.model,
                    "claude session initialized"
                );
            }
            StreamEvent::Assistant(assistant) => {
                for block in assistant.content {
                    match block {
                        AssistantContent::Text { text } => {
                            self.emit(BridgeEvent::AgentOutput {
                                workstream: ctx.workstream,
                                station: ctx.station,
                                text,
                            });
                        }
                        AssistantContent::ToolUse { name, input, .. } => {
                            self.emit(BridgeEvent::ToolCall {
                                workstream: ctx.workstream,
                                station: ctx.station,
                                tool_name: name,
                                summary: truncate(&input.to_string(), TOOL_SUMMARY_MAX_CHARS),
                            });
                        }
                        AssistantContent::Thinking | AssistantContent::Other { .. } => {}
                    }
                }
            }
            StreamEvent::UserToolResult(_) => {}
            StreamEvent::RateLimit(rl) => {
                if rl.status != "allowed" {
                    let retry_at = rl.resets_at.and_then(|secs| DateTime::from_timestamp(secs, 0));
                    let hit = RateLimitHit {
                        retry_at,
                        trigger: format!("rate_limit_event status \"{}\"", rl.status),
                    };
                    // First concrete hit wins; upgrade a missing retry_at.
                    match state.rate_limit.as_mut() {
                        None => state.rate_limit = Some(hit),
                        Some(existing) if existing.retry_at.is_none() => *existing = hit,
                        Some(_) => {}
                    }
                    self.emit(BridgeEvent::RateLimit(RateLimitState::Hit { retry_at }));
                }
            }
            StreamEvent::ApiRetry(retry) => {
                if matches!(
                    retry.error,
                    ApiErrorCategory::RateLimit | ApiErrorCategory::Overloaded
                ) {
                    state.api_retry_throttled = true;
                }
                tracing::debug!(
                    attempt = retry.attempt,
                    category = ?retry.error,
                    "claude api_retry observed"
                );
            }
            StreamEvent::Result(result) => {
                state.result = Some(result);
            }
            StreamEvent::SystemOther { subtype } => {
                tracing::trace!(subtype = %subtype, "ignoring system event");
            }
            StreamEvent::Unknown { raw_type } => {
                tracing::debug!(raw_type = %raw_type, "skipping unknown stream event type");
            }
        }
    }

    fn emit(&self, event: BridgeEvent) {
        // A send error only means nobody is listening right now.
        let _ = self.events.send(event);
    }

    fn emit_log(&self, ctx: &TurnCtx, level: LogLevel, message: String) {
        let entry = LogEntry::new(level, message)
            .station(ctx.station)
            .workstream(ctx.workstream);
        self.emit(BridgeEvent::Log(entry));
    }
}

/// Truncate on a char boundary, appending an ellipsis marker when cut.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(subtype: &str, is_error: bool, api_error_status: Option<u32>) -> ResultEvent {
        serde_json::from_value(serde_json::json!({
            "subtype": subtype,
            "is_error": is_error,
            "api_error_status": api_error_status,
        }))
        .unwrap()
    }

    fn state(
        result: Option<ResultEvent>,
        rate_limit: Option<RateLimitHit>,
        api_retry_throttled: bool,
    ) -> StreamState {
        StreamState {
            result,
            rate_limit,
            api_retry_throttled,
        }
    }

    #[test]
    fn finalize_keeps_an_existing_stream_hit() {
        let hit = RateLimitHit {
            retry_at: DateTime::from_timestamp(1785542400, 0),
            trigger: "rate_limit_event status \"rejected\"".into(),
        };
        let mut s = state(None, Some(hit.clone()), true);
        ClaudeRunner::finalize_rate_limit(&mut s, &ExitClass::NonZero(1));
        assert_eq!(s.rate_limit, Some(hit));
    }

    #[test]
    fn finalize_flags_rate_limited_looking_results() {
        let mut s = state(
            Some(result("error_during_execution", true, Some(429))),
            None,
            false,
        );
        ClaudeRunner::finalize_rate_limit(&mut s, &ExitClass::NonZero(1));
        let hit = s.rate_limit.expect("429 result should register a hit");
        assert!(hit.trigger.contains("result looks rate limited"));
    }

    #[test]
    fn finalize_flags_api_retry_throttling_only_when_the_turn_failed() {
        let mut failed = state(None, None, true);
        ClaudeRunner::finalize_rate_limit(&mut failed, &ExitClass::NonZero(1));
        assert!(failed.rate_limit.is_some());

        let mut errored_result = state(Some(result("error_during_execution", true, None)), None, true);
        ClaudeRunner::finalize_rate_limit(&mut errored_result, &ExitClass::Success);
        assert!(errored_result.rate_limit.is_some());

        let mut succeeded = state(Some(result("success", false, None)), None, true);
        ClaudeRunner::finalize_rate_limit(&mut succeeded, &ExitClass::Success);
        assert!(
            succeeded.rate_limit.is_none(),
            "recovered retries on a successful turn are not a rate limit hit"
        );
    }

    #[test]
    fn finalize_leaves_clean_turns_alone() {
        let mut s = state(Some(result("success", false, None)), None, false);
        ClaudeRunner::finalize_rate_limit(&mut s, &ExitClass::Success);
        assert!(s.rate_limit.is_none());
    }

    #[test]
    fn truncate_respects_char_boundaries_and_marks_cuts() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exact", 5), "exact");
        assert_eq!(truncate("abcdefgh", 3), "abc...");
        // Multi-byte chars must not split.
        assert_eq!(truncate("ábcdé", 2), "áb...");
    }
}
