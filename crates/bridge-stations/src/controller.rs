//! The mission controller: the state machine that runs a mission from
//! objective to final report.
//!
//! ```text
//! SayToCaptain
//!   -> Planning: a Captain conference (CaptainReply schema) converses
//!      turn by turn, proposing full plan drafts; ApproveProposal on the
//!      latest revision validates it into a MissionPlan
//!      (screen_order rejects/escalates before ANY claude spawn)
//!   -> Executing: workstreams whose dependencies are merged get spawned
//!      as tokio tasks (Ops: semaphore slots come from the engine; budget
//!      checked BEFORE each turn; rate-limit gate pauses dispatch)
//!        per workstream: provision -> Helm order(s) -> Kobayashi rounds
//!        (breached -> Helm fix in the ORIGINAL worktree -> fresh round;
//!         max rounds -> Flagged, merge only via OverrideFlagged)
//!        clean -> enqueue in MergeQueue
//!   -> merge queue (serialized; see merge_queue.rs), user confirms in GUI
//!   -> after each merge: quiet-point rebases of active workstreams
//!   -> all merged -> Comms mission report -> Complete
//!
//! Pauses: BudgetExhausted / RateLimited freeze dispatch (running turns
//! finish; sessions are preserved for --resume). ExtendBudget or a
//! successful probe resumes. WindDown finishes in-flight orders then
//! reports. Shutdown persists everything and kills children gracefully.
//! ```

use crate::amendment;
use crate::engineering::{self, ProvisionedWorktree};
use crate::kobayashi::{KobayashiError, KobayashiRunner};
use crate::merge_queue::MergeQueue;
use crate::ports::{ComputerPort, GitPort, TacticalPort, TurnPort};
use crate::profiles::station_profile;
use crate::prompts;
use bridge_compat::{ClaudeInvocation, OutputFormat};
use bridge_core::{
    BridgeCommand, BridgeConfig, BridgeEvent, BudgetExtension, BudgetSnapshot, CaptainReply,
    EscalationId, EscalationTicket, MergeProposal, MergeQueueState, MissionId, MissionPlan,
    MissionState, MissionStatusUpdate, Order, OrderId, PauseReason, PlanDraft, RateLimitState,
    SessionId, Station, UserDecision, WorkstreamId, WorkstreamSpec, WorkstreamStatus,
    WorkstreamTurns,
};
use bridge_engine::runner::ExitClass;
use bridge_engine::{EngineError, RateLimitHit, TurnCtx, TurnOutcome};
use bridge_git::{GitError, MergeOutcome, RebaseOutcome};
use bridge_tactical::ScreenResult;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Error)]
pub enum MissionError {
    #[error("command channel closed")]
    ChannelClosed,
    #[error("{0}")]
    Fatal(String),
}

/// Fallback probe delay when a rate-limit hit carries no retry-at hint.
const DEFAULT_RATE_LIMIT_PROBE: Duration = Duration::from_secs(30);

pub struct MissionController<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    deps: Arc<D>,
    config: BridgeConfig,
    helper_path: PathBuf,
    events: broadcast::Sender<BridgeEvent>,
    commands: mpsc::Receiver<BridgeCommand>,
    resumed_plan: Option<MissionPlan>,
}

impl<D> MissionController<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    pub fn new(
        deps: std::sync::Arc<D>,
        config: BridgeConfig,
        helper_path: PathBuf,
        events: broadcast::Sender<BridgeEvent>,
        commands: mpsc::Receiver<BridgeCommand>,
    ) -> Self {
        Self {
            deps,
            config,
            helper_path,
            events,
            commands,
            resumed_plan: None,
        }
    }

    /// Resume a persisted mission instead of waiting for a Captain conference.
    pub fn with_resumed_plan(mut self, plan: MissionPlan) -> Self {
        self.resumed_plan = Some(plan);
        self
    }

    /// Drive the mission loop until Shutdown or completion. Emits every
    /// state change on the event bus; consumes commands. All claude turns
    /// go through deps (mockable); all git through GitPort inside
    /// spawn_blocking.
    pub async fn run(self) -> Result<(), MissionError> {
        let (internal_tx, internal_rx) = mpsc::channel(1024);
        let shared = Arc::new(Shared {
            deps: self.deps,
            config: self.config,
            helper_path: self.helper_path,
            events: self.events,
            internal_tx,
            tasks: std::sync::Mutex::new(Vec::new()),
        });
        let mut ctl = Ctl {
            shared,
            commands: self.commands,
            internal_rx,
            conference: None,
            mission: None,
            winding_down: false,
            exit: None,
        };
        if let Some(plan) = self.resumed_plan {
            ctl.begin_mission(plan).await;
        }
        ctl.event_loop().await
    }
}

// ---------------------------------------------------------------------------
// runtime state

struct Shared<D> {
    deps: Arc<D>,
    config: BridgeConfig,
    helper_path: PathBuf,
    events: broadcast::Sender<BridgeEvent>,
    internal_tx: mpsc::Sender<Internal>,
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<D> Shared<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    fn emit(&self, ev: BridgeEvent) {
        let _ = self.events.send(ev);
    }

    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        self.tasks.lock().unwrap().push(tokio::spawn(fut));
    }

    /// Run a blocking git/engineering closure on the blocking pool.
    async fn blocking<R: Send + 'static>(&self, f: impl FnOnce(&D) -> R + Send + 'static) -> R {
        let deps = self.deps.clone();
        tokio::task::spawn_blocking(move || f(&deps))
            .await
            .expect("blocking task panicked")
    }

    fn abort_tasks(&self) {
        for h in self.tasks.lock().unwrap().drain(..) {
            h.abort();
        }
    }
}

/// Work that consumes an agent turn; every dispatch passes the budget and
/// pause gates first.
#[derive(Debug)]
enum Dispatch {
    Helm {
        ws: WorkstreamId,
        purpose: TurnPurpose,
        order: Order,
    },
    Kobayashi {
        ws: WorkstreamId,
        round: u32,
    },
    Comms,
}

impl Dispatch {
    fn ws(&self) -> Option<WorkstreamId> {
        match self {
            Dispatch::Helm { ws, .. } | Dispatch::Kobayashi { ws, .. } => Some(*ws),
            Dispatch::Comms => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnPurpose {
    /// The initial execution order for the workstream brief.
    Work,
    /// Fix findings after a Kobayashi breach.
    FixBreach,
    /// Resolve rebase conflicts; `in_queue` = part of merge-queue
    /// processing (a fresh Kobayashi round must follow).
    FixConflicts { in_queue: bool },
}

enum Internal {
    CaptainDone {
        result: Result<TurnOutcome, EngineError>,
    },
    TurnDone {
        ws: WorkstreamId,
        purpose: TurnPurpose,
        order_id: OrderId,
        started_at: DateTime<Utc>,
        result: Result<TurnOutcome, EngineError>,
    },
    KobayashiDone {
        ws: WorkstreamId,
        round: u32,
        result: Result<bridge_core::BattleReport, KobayashiError>,
    },
    RebaseDone {
        ws: WorkstreamId,
        in_queue: bool,
        result: Result<RebaseData, GitError>,
    },
    MergeDone {
        ws: WorkstreamId,
        result: Result<MergeOutcome, GitError>,
    },
    CommsDone {
        order_id: OrderId,
        started_at: DateTime<Utc>,
        result: Result<TurnOutcome, EngineError>,
    },
    /// Deferred Comms dispatch (breaks recursion through maybe_finish).
    DispatchComms,
    RateLimitProbe,
    /// A pre-dispatch screening escalation hit its deadline unanswered.
    EscalationTimeout {
        id: EscalationId,
    },
}

struct RebaseData {
    outcome: RebaseOutcome,
    diff_stat: Option<String>,
    target: String,
}

struct Conference {
    mission_id: MissionId,
    slug: String,
    objective: String,
    session: Option<SessionId>,
    revision: u64,
    latest_proposal: Option<(u64, PlanDraft)>,
    /// Some((order_id, started_at)) while a Captain turn is running.
    turn_in_flight: Option<(OrderId, DateTime<Utc>)>,
    queued: VecDeque<String>,
    turns_used: u32,
    auto_retries: u32,
    /// Approval locked while a turn was in flight; its proposal is discarded.
    approved: bool,
}

struct WsState {
    spec: WorkstreamSpec,
    status: WorkstreamStatus,
    provisioned: Option<ProvisionedWorktree>,
    helm_session: Option<SessionId>,
    turns: u32,
    /// Kobayashi rounds completed so far.
    rounds: u32,
    /// A turn, round or rebase task is in flight.
    busy: bool,
    started: bool,
    /// The current fix/test cycle belongs to merge-queue processing.
    merge_gate: bool,
    /// Rebase onto new main is owed at the next quiet point.
    pending_rebase: bool,
    /// Next action stashed while a quiet-point rebase runs.
    stashed: Option<Dispatch>,
}

struct Mission {
    plan: MissionPlan,
    /// Synthetic workstream id for mission-level (Captain/Comms) turns.
    captain_ws: WorkstreamId,
    started_at: DateTime<Utc>,
    ws: HashMap<WorkstreamId, WsState>,
    /// Topological iteration order.
    order: Vec<WorkstreamId>,
    queue: MergeQueue,
    seq: u64,
    total_turns: u32,
    total_cost: f64,
    max_total_turns: u32,
    max_turns_per_ws: u32,
    max_wall_clock_secs: Option<u64>,
    paused: Option<PauseReason>,
    pending: VecDeque<Dispatch>,
    /// Open order-screening escalations: the ticket (retained so it can be
    /// re-emitted on a resync) and the dispatch it is gating.
    escalations: HashMap<EscalationId, (EscalationTicket, Dispatch)>,
    /// Merge proposals awaiting user confirmation, retained for resync.
    pending_merges: HashMap<WorkstreamId, MergeProposal>,
    comms_dispatched: bool,
}

struct Ctl<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    shared: Arc<Shared<D>>,
    commands: mpsc::Receiver<BridgeCommand>,
    internal_rx: mpsc::Receiver<Internal>,
    conference: Option<Conference>,
    mission: Option<Mission>,
    winding_down: bool,
    exit: Option<Result<(), MissionError>>,
}

/// Kebab-case slug from a free-form objective, truncated, never empty.
fn slugify(objective: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true;
    for c in objective.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "mission".to_string()
    } else {
        trimmed.to_string()
    }
}

fn truncate_summary(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{cut}...")
    }
}

impl<D> Ctl<D>
where
    D: TurnPort + GitPort + TacticalPort + ComputerPort,
{
    async fn event_loop(mut self) -> Result<(), MissionError> {
        let exit = loop {
            tokio::select! {
                cmd = self.commands.recv() => match cmd {
                    Some(c) => self.handle_command(c).await,
                    None => {
                        // GUI hung up: graceful shutdown.
                        break if self.mission.is_some() || self.conference.is_some() {
                            Err(MissionError::ChannelClosed)
                        } else {
                            Ok(())
                        };
                    }
                },
                Some(msg) = self.internal_rx.recv() => self.handle_internal(msg).await,
            }
            if let Some(exit) = self.exit.take() {
                break exit;
            }
        };
        self.shared.abort_tasks();
        // Single cleanup point: tear down every provisioned worktree and its
        // installed hooks + Tactical arming. Merged work is always removed;
        // unmerged work is kept only when keep_on_failure is set, so a user
        // can inspect a failed or shut-down mission's worktrees.
        self.decommission_all().await;
        exit
    }

    /// Decommission all still-provisioned workstream worktrees. Best-effort:
    /// engineering::decommission logs rather than propagates.
    async fn decommission_all(&mut self) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        let keep_on_failure = self.shared.config.worktrees.keep_on_failure;
        let jobs: Vec<(WorkstreamId, ProvisionedWorktree, bool)> =
            m.ws.iter_mut()
                .filter_map(|(id, w)| {
                    let provisioned = w.provisioned.take()?;
                    let merged = matches!(w.status, WorkstreamStatus::Merged);
                    Some((*id, provisioned, !merged && keep_on_failure))
                })
                .collect();
        for (ws, provisioned, keep) in jobs {
            self.shared
                .blocking(move |d| engineering::decommission(d, d, ws, &provisioned, keep))
                .await;
        }
    }

    async fn handle_command(&mut self, cmd: BridgeCommand) {
        match cmd {
            BridgeCommand::SayToCaptain { text } => self.say_to_captain(text).await,
            BridgeCommand::ApproveProposal { revision } => self.approve_proposal(revision).await,
            BridgeCommand::ResolveEscalation { id, decision } => {
                self.resolve_escalation(id, decision).await
            }
            BridgeCommand::ConfirmMerge {
                workstream,
                approved,
            } => self.confirm_merge(workstream, approved).await,
            BridgeCommand::ExtendBudget(ext) => self.extend_budget(ext).await,
            BridgeCommand::WindDown => {
                self.winding_down = true;
                if let Some(m) = self.mission.as_ref() {
                    self.shared
                        .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                            mission: m.plan.mission_id,
                            state: MissionState::WindingDown,
                            detail: None,
                        }));
                }
                self.maybe_finish();
            }
            BridgeCommand::OverrideFlagged { workstream } => {
                self.override_flagged(workstream).await
            }
            BridgeCommand::Shutdown => {
                self.exit = Some(Ok(()));
            }
            BridgeCommand::ResyncActionable => self.resync_actionable(),
        }
    }

    async fn handle_internal(&mut self, msg: Internal) {
        match msg {
            Internal::CaptainDone { result } => self.captain_done(result).await,
            Internal::TurnDone {
                ws,
                purpose,
                order_id,
                started_at,
                result,
            } => {
                self.turn_done(ws, purpose, order_id, started_at, result)
                    .await
            }
            Internal::KobayashiDone { ws, round, result } => {
                self.kobayashi_done(ws, round, result).await
            }
            Internal::RebaseDone {
                ws,
                in_queue,
                result,
            } => self.rebase_done(ws, in_queue, result).await,
            Internal::MergeDone { ws, result } => self.merge_done(ws, result).await,
            Internal::CommsDone {
                order_id,
                started_at,
                result,
            } => self.comms_done(order_id, started_at, result).await,
            Internal::DispatchComms => self.dispatch_now(Dispatch::Comms).await,
            Internal::RateLimitProbe => self.rate_limit_probe().await,
            Internal::EscalationTimeout { id } => self.escalation_timeout(id).await,
        }
    }

    /// A pre-dispatch screening escalation went unanswered for its timeout;
    /// fail closed to Deny so the mission cannot wedge on it. A no-op if the
    /// user already resolved it (id no longer pending).
    async fn escalation_timeout(&mut self, id: EscalationId) {
        let still_pending = self
            .mission
            .as_ref()
            .is_some_and(|m| m.escalations.contains_key(&id));
        if still_pending {
            tracing::warn!("screening escalation {id} timed out; denying (fail closed)");
            self.resolve_escalation(
                id,
                UserDecision::Deny {
                    reason: "screening escalation timed out".into(),
                },
            )
            .await;
        }
    }

    // -- captain conference -------------------------------------------------

    /// Pre-launch phase (no active mission): open or continue the
    /// conference. Mid-mission (a running mission with no conference open
    /// yet, e.g. a resumed mission): open a fresh conference, hydrating its
    /// session or falling back to a recap (Task 7 adds the amendment arm).
    async fn say_to_captain(&mut self, text: String) {
        if self.mission.is_some() && self.conference.is_none() {
            self.open_mid_mission_conference(text).await;
            return;
        }
        if self.conference.is_none() {
            let mission_id = MissionId::new();
            let slug = slugify(&text);
            self.shared
                .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                    mission: mission_id,
                    state: MissionState::Planning,
                    detail: Some(text.clone()),
                }));
            // Placeholder row so conference turns have a mission to attach
            // to; begin_mission's record_mission upserts the real plan later.
            let placeholder = MissionPlan {
                mission_id,
                slug: slug.clone(),
                objective: text.clone(),
                workstreams: Vec::new(),
                edges: Vec::new(),
            };
            if let Err(e) = self
                .shared
                .deps
                .record_mission(&placeholder, &self.shared.config)
            {
                tracing::warn!("failed to persist conference mission: {e}");
            }
            self.conference = Some(Conference {
                mission_id,
                slug,
                objective: text.clone(),
                session: None,
                revision: 0,
                latest_proposal: None,
                turn_in_flight: None,
                queued: VecDeque::new(),
                turns_used: 0,
                auto_retries: 0,
                approved: false,
            });
            let mission = mission_id;
            self.shared.emit(BridgeEvent::UserSaid {
                mission,
                text: text.clone(),
            });
            let main = self
                .shared
                .blocking(|d| d.main_branch())
                .await
                .unwrap_or_else(|_| "main".into());
            let opening = prompts::captain_confer_opening(
                &text,
                &format!("Target repository main branch: {main}."),
            );
            self.start_captain_turn(opening);
            return;
        }
        let conf = self.conference.as_mut().expect("checked above");
        self.shared.emit(BridgeEvent::UserSaid {
            mission: conf.mission_id,
            text: text.clone(),
        });
        if conf.turn_in_flight.is_some() || conf.approved {
            conf.queued.push_back(text);
        } else {
            // Any messages queued behind a now-finished turn (e.g. one that
            // failed - the failure arms return before the queue flush) must
            // be concatenated ahead of this new text, in order, rather than
            // sent alone while the queued ones straggle in later.
            conf.queued.push_back(text);
            let msg = conf.queued.drain(..).collect::<Vec<_>>().join("\n\n");
            self.start_captain_turn(msg);
        }
    }

    /// Open a Captain conference for a mission that is already running (no
    /// pre-launch conference exists, e.g. a resumed mission). Hydrates the
    /// Captain's session if one survives for this repo root; otherwise
    /// prefixes the first turn with a recap of the objective and current
    /// workstream statuses so the Captain has full context.
    async fn open_mid_mission_conference(&mut self, first_text: String) {
        let Some(m) = self.mission.as_ref() else {
            return;
        };
        let mission_id = m.plan.mission_id;
        let captain_ws = m.captain_ws;
        let slug = m.plan.slug.clone();
        let objective = m.plan.objective.clone();
        let root = self.shared.deps.repo_root();
        let session = match self.shared.deps.session_for(captain_ws, Station::Captain) {
            Ok(Some((sid, cwd))) if cwd == root => Some(sid),
            _ => None,
        };
        let recap = if session.is_none() {
            let statuses: Vec<(String, String)> = m
                .plan
                .workstreams
                .iter()
                .map(|w| {
                    let s =
                        m.ws.get(&w.id)
                            .map(|ws| format!("{:?}", ws.status))
                            .unwrap_or_else(|| "Unknown".into());
                    (w.slug.clone(), s)
                })
                .collect();
            Some(prompts::captain_recap(&objective, &m.plan, &statuses))
        } else {
            None
        };
        self.conference = Some(Conference {
            mission_id,
            slug,
            objective,
            session,
            revision: 0,
            latest_proposal: None,
            turn_in_flight: None,
            queued: VecDeque::new(),
            turns_used: 0,
            auto_retries: 0,
            approved: false,
        });
        self.shared.emit(BridgeEvent::UserSaid {
            mission: mission_id,
            text: first_text.clone(),
        });
        let message = match recap {
            Some(r) => format!("{r}\n\n## Your officer says\n{first_text}"),
            None => first_text,
        };
        self.start_captain_turn(message);
    }

    /// Mirrors the old `start_mission` invocation body: same profile, tools
    /// and streaming, but the CaptainReply schema, a resumed session, and a
    /// per-mission conference turn cap.
    fn start_captain_turn(&mut self, message: String) {
        let Some(conf) = self.conference.as_mut() else {
            return;
        };
        if conf.turns_used >= self.shared.config.captain.max_conference_turns {
            let revision = conf.revision;
            let mission = conf.mission_id;
            self.shared.emit(BridgeEvent::ProposalRejected {
                mission,
                revision,
                reason: "conference turn budget exhausted; approve the latest proposal, extend captain.max_conference_turns, or shut down".into(),
            });
            return;
        }
        conf.turns_used += 1;
        let profile = station_profile(Station::Captain, &self.shared.config);
        let order_id = OrderId::new();
        let inv = ClaudeInvocation {
            prompt: message,
            cwd: self.shared.deps.repo_root(),
            resume: conf.session.clone(),
            max_turns: profile.max_turns_default,
            allowed_tools: profile.allowed_tools.clone(),
            disallowed_tools: profile.disallowed_tools.clone(),
            model: Some(profile.model.clone()),
            json_schema: Some(CaptainReply::json_schema()),
            mcp_config: self
                .shared
                .config
                .linear
                .as_ref()
                .map(|l| l.mcp_config_path.clone()),
            append_system_prompt: Some(profile.append_system_prompt.clone()),
            output_format: OutputFormat::StreamJson,
            setting_sources: Vec::new(),
        };
        let ctx = TurnCtx {
            workstream: WorkstreamId(conf.mission_id.0),
            station: Station::Captain,
            kobayashi: false,
            pid_register: None,
        };
        conf.turn_in_flight = Some((order_id, Utc::now()));
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let result = deps.run_turn(inv, ctx).await;
            let _ = tx.send(Internal::CaptainDone { result }).await;
        });
    }

    async fn captain_done(&mut self, result: Result<TurnOutcome, EngineError>) {
        let Some(conf) = self.conference.as_mut() else {
            return;
        };
        let Some((order_id, started_at)) = conf.turn_in_flight.take() else {
            return;
        };
        let mission = conf.mission_id;

        let outcome = match result {
            Ok(o)
                if o.exit == ExitClass::Success
                    && !o.result.as_ref().is_some_and(|r| r.is_error) =>
            {
                o
            }
            Ok(_) => {
                self.shared.emit(BridgeEvent::CaptainSays {
                    mission,
                    text: "The Captain did not respond cleanly; say something to retry.".into(),
                });
                return;
            }
            Err(e) => {
                self.shared.emit(BridgeEvent::CaptainSays {
                    mission,
                    text: format!("The Captain did not respond: {e}. Say something to retry."),
                });
                return;
            }
        };

        // Session continuity: capture and persist. NOTE the borrow dance:
        // `conf` (a &mut into self.conference) must be dropped before any
        // `&mut self` method call, and re-fetched afterwards.
        if let Some(sid) = outcome.result.as_ref().and_then(|r| r.session_id.clone()) {
            let sid = SessionId(sid);
            conf.session = Some(sid.clone());
            let ws = WorkstreamId(mission.0);
            let root = self.shared.deps.repo_root();
            if let Err(e) = self
                .shared
                .deps
                .record_session(ws, Station::Captain, &sid, &root)
            {
                tracing::warn!("failed to persist captain session: {e}");
            }
        }
        self.record_conference_turn(order_id, started_at, &outcome); // &mut self: conf dropped here

        let structured = outcome.structured_output.clone().or_else(|| {
            outcome
                .result
                .as_ref()
                .and_then(|r| r.structured_output.clone())
        });
        let parsed: Option<CaptainReply> = structured.and_then(|v| serde_json::from_value(v).ok());
        let Some(conf) = self.conference.as_mut() else {
            return;
        }; // re-fetch
        let reply: CaptainReply = match parsed {
            Some(reply) => reply,
            None if conf.auto_retries < 1 => {
                conf.auto_retries += 1;
                self.start_captain_turn(prompts::SCHEMA_RETRY_MSG.into());
                return;
            }
            None => {
                conf.auto_retries = 0;
                self.shared.emit(BridgeEvent::CaptainSays {
                    mission,
                    text: "The Captain's reply was unreadable twice; say something to retry."
                        .into(),
                });
                return;
            }
        };
        self.shared.emit(BridgeEvent::CaptainSays {
            mission,
            text: reply.message.clone(),
        });

        // One-shot: the lock exists only to discard the proposal of THIS
        // in-flight turn. Clear it here regardless of whether a proposal
        // actually arrived - a reply that came back conversational-only
        // must not leave the lock wedged shut (it would otherwise queue
        // every future turn, including the very mid-mission conference
        // this lock was protecting, forever).
        let was_locked = self.conference.as_ref().is_some_and(|c| c.approved);
        if was_locked {
            if let Some(conf) = self.conference.as_mut() {
                conf.approved = false;
            }
            if reply.proposed_plan.is_some() {
                tracing::info!("proposal discarded: approval already locked");
            }
        } else if let Some(draft) = reply.proposed_plan {
            self.handle_proposal(draft).await; // pre-launch validation below
        }

        // Flush queued user messages as one concatenated turn.
        if let Some(conf) = self.conference.as_mut()
            && !conf.queued.is_empty()
            && !conf.approved
            && conf.turn_in_flight.is_none()
        {
            let msg = conf.queued.drain(..).collect::<Vec<_>>().join("\n\n");
            self.start_captain_turn(msg);
        }
        // Completion recheck: this turn may have been the last thing
        // blocking `maybe_finish` (e.g. the mission's final workstream
        // merged while this Captain turn was in flight). Idempotent:
        // `comms_dispatched` guards double dispatch, and a flush-started
        // turn above re-blocks correctly via `turn_in_flight`.
        self.maybe_finish();
    }

    /// Pre-launch (no running mission): full-plan validation. Mid-mission
    /// (a running mission): diff-and-lock amendment validation.
    async fn handle_proposal(&mut self, draft: PlanDraft) {
        if self.mission.is_some() {
            self.handle_amendment_proposal(draft).await;
        } else {
            self.handle_prelaunch_proposal(draft).await;
        }
    }

    async fn handle_prelaunch_proposal(&mut self, draft: PlanDraft) {
        let Some(conf) = self.conference.as_mut() else {
            return;
        };
        let mission = conf.mission_id;
        // Pre-launch: validate structurally by building the real plan.
        let base_ref = self
            .shared
            .blocking(|d| d.main_branch())
            .await
            .unwrap_or_else(|_| "main".into());
        match MissionPlan::from_draft(
            draft.clone(),
            mission,
            &conf.slug,
            &conf.objective,
            &base_ref,
        ) {
            Ok(_) => {
                conf.auto_retries = 0;
                conf.revision += 1;
                conf.latest_proposal = Some((conf.revision, draft.clone()));
                self.shared.emit(BridgeEvent::PlanProposed {
                    mission,
                    revision: conf.revision,
                    plan: draft,
                    diff: None,
                });
            }
            Err(e) if conf.auto_retries < 2 => {
                conf.auto_retries += 1;
                self.start_captain_turn(format!("{}{e}", prompts::PLAN_REJECTED_PREFIX));
            }
            Err(e) => {
                conf.auto_retries = 0;
                let revision = conf.revision;
                self.shared.emit(BridgeEvent::ProposalRejected {
                    mission,
                    revision,
                    reason: e.to_string(),
                });
            }
        }
    }

    /// Mid-mission: validate the re-emitted full draft against the running
    /// plan's lock rules (`amendment::validate_amendment`); on success the
    /// proposal carries a `PlanDiff` for the GUI/user to review before
    /// approving.
    async fn handle_amendment_proposal(&mut self, draft: PlanDraft) {
        let Some(m) = self.mission.as_ref() else {
            return;
        };
        let statuses: HashMap<WorkstreamId, WorkstreamStatus> =
            m.ws.iter().map(|(id, w)| (*id, w.status.clone())).collect();
        let result = amendment::validate_amendment(&m.plan, &statuses, &draft);
        let Some(conf) = self.conference.as_mut() else {
            return;
        };
        let mission = conf.mission_id;
        match result {
            Ok(diff) => {
                conf.auto_retries = 0;
                conf.revision += 1;
                conf.latest_proposal = Some((conf.revision, draft.clone()));
                self.shared.emit(BridgeEvent::PlanProposed {
                    mission,
                    revision: conf.revision,
                    plan: draft,
                    diff: Some(diff),
                });
            }
            Err(e) if conf.auto_retries < 2 => {
                conf.auto_retries += 1;
                self.start_captain_turn(format!("{}{e}", prompts::AMENDMENT_REJECTED_PREFIX));
            }
            Err(e) => {
                conf.auto_retries = 0;
                let revision = conf.revision;
                self.shared.emit(BridgeEvent::ProposalRejected {
                    mission,
                    revision,
                    reason: e.to_string(),
                });
            }
        }
    }

    async fn approve_proposal(&mut self, revision: u64) {
        let Some(conf) = self.conference.as_mut() else {
            tracing::warn!("ApproveProposal ignored: no conference");
            return;
        };
        let mission = conf.mission_id;
        match &conf.latest_proposal {
            Some((rev, _)) if *rev == revision => {}
            _ => {
                self.shared.emit(BridgeEvent::ProposalRejected {
                    mission,
                    revision,
                    reason: "stale or unknown proposal revision".into(),
                });
                return;
            }
        }
        if conf.turn_in_flight.is_some() {
            conf.approved = true; // in-flight turn's proposal will be discarded
        }
        let (_, draft) = conf.latest_proposal.clone().expect("checked above");
        if self.mission.is_none() {
            let (slug, objective) = (conf.slug.clone(), conf.objective.clone());
            let base_ref = self
                .shared
                .blocking(|d| d.main_branch())
                .await
                .unwrap_or_else(|_| "main".into());
            match MissionPlan::from_draft(draft, mission, &slug, &objective, &base_ref) {
                Ok(plan) => {
                    if let Some(c) = self.conference.as_mut() {
                        c.latest_proposal = None;
                    }
                    self.begin_mission(plan).await;
                }
                Err(e) => {
                    self.shared.emit(BridgeEvent::ProposalRejected {
                        mission,
                        revision,
                        reason: format!("invalid plan: {e}"),
                    });
                }
            }
        } else {
            self.apply_amendment(draft, revision).await;
        }
    }

    /// Approved mid-mission amendment: re-validate (state may have drifted
    /// since the proposal was raised - e.g. a Pending workstream may have
    /// started in the meantime), then splice the diff into the running
    /// plan: locked specs pass through verbatim, revised Pending specs take
    /// the draft's title/description, removed Pending workstreams are
    /// cancelled (their `WsState` history is kept), and added workstreams
    /// get a fresh `WsState` and are announced Pending.
    async fn apply_amendment(&mut self, draft: PlanDraft, revision: u64) {
        let Some(m) = self.mission.as_ref() else {
            return;
        };
        let mission = m.plan.mission_id;
        let statuses: HashMap<WorkstreamId, WorkstreamStatus> =
            m.ws.iter().map(|(id, w)| (*id, w.status.clone())).collect();
        let diff = match amendment::validate_amendment(&m.plan, &statuses, &draft) {
            Ok(diff) => diff,
            Err(e) => {
                self.shared.emit(BridgeEvent::ProposalRejected {
                    mission,
                    revision,
                    reason: e,
                });
                return;
            }
        };
        let needs_base_ref_fallback = m.plan.workstreams.is_empty();
        let base_ref_fallback = if needs_base_ref_fallback {
            Some(
                self.shared
                    .blocking(|d| d.main_branch())
                    .await
                    .unwrap_or_else(|_| "main".into()),
            )
        } else {
            None
        };

        let (removed_ids, added_specs) = {
            let m = self.mission.as_mut().expect("checked above");
            let base_ref =
                base_ref_fallback.unwrap_or_else(|| m.plan.workstreams[0].base_ref.clone());

            let mut slug_to_id: HashMap<String, WorkstreamId> = m
                .plan
                .workstreams
                .iter()
                .map(|w| (w.slug.clone(), w.id))
                .collect();
            for dw in &draft.workstreams {
                slug_to_id.entry(dw.slug.clone()).or_default();
            }

            let mut existing: HashMap<String, WorkstreamSpec> = m
                .plan
                .workstreams
                .drain(..)
                .map(|w| (w.slug.clone(), w))
                .collect();
            let mut new_workstreams = Vec::with_capacity(draft.workstreams.len());
            let mut added_specs = Vec::new();
            for dw in &draft.workstreams {
                let id = slug_to_id[&dw.slug];
                let spec = match existing.remove(&dw.slug) {
                    Some(mut spec) => {
                        spec.title = dw.title.clone();
                        spec.description = dw.description.clone();
                        spec
                    }
                    None => {
                        let spec = WorkstreamSpec {
                            id,
                            slug: dw.slug.clone(),
                            title: dw.title.clone(),
                            description: dw.description.clone(),
                            base_ref: base_ref.clone(),
                        };
                        added_specs.push(spec.clone());
                        spec
                    }
                };
                new_workstreams.push(spec);
            }
            // Whatever is left in `existing` had a slug the draft dropped:
            // exactly the removed set (locked removals already failed
            // re-validation above).
            let removed_ids: Vec<WorkstreamId> = existing.values().map(|w| w.id).collect();
            m.plan.edges = draft
                .workstreams
                .iter()
                .flat_map(|dw| {
                    let to = slug_to_id[&dw.slug];
                    dw.depends_on
                        .iter()
                        .map(|dep| (slug_to_id[dep], to))
                        .collect::<Vec<_>>()
                })
                .collect();
            m.plan.workstreams = new_workstreams;
            // Keep each surviving WsState's cached spec (title/description
            // used to build the Helm order later) in sync with the plan.
            let synced: Vec<WorkstreamSpec> = m.plan.workstreams.clone();
            for spec in synced {
                if let Some(w) = m.ws.get_mut(&spec.id) {
                    w.spec = spec;
                }
            }
            (removed_ids, added_specs)
        };

        for id in removed_ids {
            self.set_status(id, WorkstreamStatus::Cancelled);
            if let Some(m) = self.mission.as_mut()
                && let Some(w) = m.ws.get_mut(&id)
            {
                w.started = true;
            }
        }

        for spec in added_specs {
            let id = spec.id;
            if let Some(m) = self.mission.as_mut() {
                m.ws.insert(
                    id,
                    WsState {
                        spec,
                        status: WorkstreamStatus::Pending,
                        provisioned: None,
                        helm_session: None,
                        turns: 0,
                        rounds: 0,
                        busy: false,
                        started: false,
                        merge_gate: false,
                        pending_rebase: false,
                        stashed: None,
                    },
                );
            }
            if let Err(e) = self
                .shared
                .deps
                .record_workstream_status(id, &WorkstreamStatus::Pending)
            {
                tracing::warn!("failed to record workstream status: {e}");
            }
            self.shared.emit(BridgeEvent::WorkstreamStatus {
                id,
                status: WorkstreamStatus::Pending,
            });
        }

        let Some(m) = self.mission.as_mut() else {
            return;
        };
        m.order = m
            .plan
            .topo_order()
            .unwrap_or_else(|_| m.plan.workstreams.iter().map(|w| w.id).collect());
        m.queue.set_topo(m.order.clone());
        if let Err(e) = self
            .shared
            .deps
            .record_mission(&m.plan, &self.shared.config)
        {
            tracing::warn!("failed to persist amended mission: {e}");
        }
        if let Some(c) = self.conference.as_mut() {
            c.latest_proposal = None;
            // Mirror the launch arm: only clear the approval lock when no
            // turn is in flight. An in-flight turn's late proposal is
            // discarded by captain_done's own hoisted reset once that turn
            // resolves; clearing it here too early would let the late
            // proposal fall through to handle_proposal instead.
            if c.turn_in_flight.is_none() {
                c.approved = false;
            }
        }
        self.start_eligible().await;
        // Positive signal that the amendment actually applied: without this
        // the GUI's proposal card only clears on Executing, and wire
        // consumers have no event to observe the application by.
        self.shared
            .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                mission,
                state: MissionState::Executing,
                detail: Some(format!(
                    "plan amended: +{} ~{} x{}",
                    diff.added.len(),
                    diff.revised.len(),
                    diff.removed.len()
                )),
            }));
    }

    /// Install the plan as the running mission.
    async fn begin_mission(&mut self, plan: MissionPlan) {
        let order = plan
            .topo_order()
            .unwrap_or_else(|_| plan.workstreams.iter().map(|w| w.id).collect());
        let captain_ws = WorkstreamId(plan.mission_id.0);
        let ws = plan
            .workstreams
            .iter()
            .map(|spec| {
                (
                    spec.id,
                    WsState {
                        spec: spec.clone(),
                        status: WorkstreamStatus::Pending,
                        provisioned: None,
                        helm_session: None,
                        turns: 0,
                        rounds: 0,
                        busy: false,
                        started: false,
                        merge_gate: false,
                        pending_rebase: false,
                        stashed: None,
                    },
                )
            })
            .collect();
        if let Err(e) = self.shared.deps.record_mission(&plan, &self.shared.config) {
            tracing::warn!("failed to persist mission: {e}");
        }
        let mission = Mission {
            captain_ws,
            started_at: Utc::now(),
            ws,
            order,
            queue: MergeQueue::new(
                plan.topo_order()
                    .unwrap_or_else(|_| plan.workstreams.iter().map(|w| w.id).collect()),
            ),
            seq: 0,
            total_turns: 0,
            total_cost: 0.0,
            max_total_turns: self.shared.config.budgets.max_total_turns,
            max_turns_per_ws: self.shared.config.budgets.max_turns_per_workstream,
            max_wall_clock_secs: self.shared.config.budgets.max_wall_clock_secs,
            paused: None,
            pending: VecDeque::new(),
            escalations: HashMap::new(),
            pending_merges: HashMap::new(),
            comms_dispatched: false,
            plan,
        };
        let mission_id = mission.plan.mission_id;
        self.mission = Some(mission);
        self.shared
            .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                mission: mission_id,
                state: MissionState::Executing,
                detail: None,
            }));
        // Announce the initial Pending status explicitly (set_status only
        // reports changes, and workstreams are born Pending).
        let ids: Vec<WorkstreamId> = self.mission.as_ref().unwrap().order.clone();
        for id in ids {
            if let Err(e) = self
                .shared
                .deps
                .record_workstream_status(id, &WorkstreamStatus::Pending)
            {
                tracing::warn!("failed to record workstream status: {e}");
            }
            self.shared.emit(BridgeEvent::WorkstreamStatus {
                id,
                status: WorkstreamStatus::Pending,
            });
        }
        self.start_eligible().await;
    }

    // -- workstream lifecycle ----------------------------------------------

    /// Start every unstarted workstream whose dependencies are all merged;
    /// fail those whose dependencies can never merge.
    async fn start_eligible(&mut self) {
        if !self.winding_down {
            loop {
                let (to_fail, to_start) = {
                    let Some(m) = self.mission.as_ref() else {
                        return;
                    };
                    let mut to_fail = Vec::new();
                    let mut to_start = Vec::new();
                    for &id in &m.order {
                        let w = &m.ws[&id];
                        if w.started {
                            continue;
                        }
                        let deps = m.plan.dependencies_of(id);
                        if deps
                            .iter()
                            .any(|d| matches!(m.ws[d].status, WorkstreamStatus::Failed { .. }))
                        {
                            to_fail.push(id);
                        } else if deps
                            .iter()
                            .all(|d| m.ws[d].status == WorkstreamStatus::Merged)
                        {
                            to_start.push(id);
                        }
                    }
                    (to_fail, to_start)
                };
                if to_fail.is_empty() && to_start.is_empty() {
                    break;
                }
                for id in to_fail {
                    if let Some(m) = self.mission.as_mut()
                        && let Some(w) = m.ws.get_mut(&id)
                    {
                        w.started = true;
                    }
                    self.set_status(
                        id,
                        WorkstreamStatus::Failed {
                            reason: "a dependency failed".into(),
                        },
                    );
                }
                for id in to_start {
                    self.start_workstream(id).await;
                }
            }
        }
        self.maybe_finish();
    }

    async fn start_workstream(&mut self, id: WorkstreamId) {
        let (spec, objective) = {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            let Some(w) = m.ws.get_mut(&id) else { return };
            w.started = true;
            (w.spec.clone(), m.plan.objective.clone())
        };
        let profile = station_profile(Station::Helm, &self.shared.config);
        let mut order = Order::new(id, Station::Helm, prompts::helm_order(&spec, &objective));
        order.max_turns = profile.max_turns_default;
        order.allowed_tools = profile.allowed_tools.clone();
        order.disallowed_tools = profile.disallowed_tools.clone();
        self.screen_and_dispatch(
            Dispatch::Helm {
                ws: id,
                purpose: TurnPurpose::Work,
                order,
            },
            &profile.allowed_tools,
        )
        .await;
    }

    /// Tactical pre-dispatch screening for Helm orders: Rejected fails the
    /// workstream before any claude spawn; NeedsReview holds the dispatch
    /// behind an escalation ticket.
    async fn screen_and_dispatch(&mut self, dispatch: Dispatch, station_allowed: &[String]) {
        let Dispatch::Helm { ws, ref order, .. } = dispatch else {
            self.try_dispatch(dispatch).await;
            return;
        };
        match self.shared.deps.screen_order(order, station_allowed) {
            ScreenResult::Cleared => self.try_dispatch(dispatch).await,
            ScreenResult::Rejected { reason } => {
                self.set_status(
                    ws,
                    WorkstreamStatus::Failed {
                        reason: format!("order rejected by Tactical: {reason}"),
                    },
                );
                self.abandon_queue_entry(ws).await;
                self.maybe_finish();
            }
            ScreenResult::NeedsReview { flags } => {
                let timeout = self.shared.config.tactical.escalation_timeout_secs;
                let ticket = EscalationTicket {
                    id: EscalationId::new(),
                    workstream: ws,
                    question: format!(
                        "Pre-dispatch screening flagged this order: {}",
                        flags.join("; ")
                    ),
                    tool_name: None,
                    tool_input_summary: truncate_summary(&order.prompt, 200),
                    requested_at: Utc::now(),
                    expires_at: Utc::now() + chrono::Duration::seconds(timeout as i64),
                };
                let id = ticket.id;
                {
                    let Some(m) = self.mission.as_mut() else {
                        return;
                    };
                    m.escalations.insert(id, (ticket.clone(), dispatch));
                }
                self.shared.emit(BridgeEvent::EscalationRequested(ticket));
                // Fail closed if the user never answers: after the timeout,
                // deny the order rather than wedging the mission.
                let tx = self.shared.internal_tx.clone();
                self.shared.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(timeout)).await;
                    let _ = tx.send(Internal::EscalationTimeout { id }).await;
                });
            }
        }
    }

    // -- dispatch gates -----------------------------------------------------

    /// Budget and pause gate in front of every agent turn.
    async fn try_dispatch(&mut self, dispatch: Dispatch) {
        if self.winding_down && !matches!(dispatch, Dispatch::Comms) {
            tracing::info!("winding down; dropping dispatch {dispatch:?}");
            self.maybe_finish();
            return;
        }
        {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            if m.paused.is_some() {
                m.pending.push_back(dispatch);
                return;
            }
        }
        let exhausted: Option<String> = {
            let m = self.mission.as_ref().unwrap();
            if m.total_turns >= m.max_total_turns {
                Some("max_total_turns".to_string())
            } else if let Some(max) = m.max_wall_clock_secs
                && (Utc::now() - m.started_at).num_seconds().max(0) as u64 >= max
            {
                Some("max_wall_clock_secs".to_string())
            } else if let Some(ws) = dispatch.ws()
                && let Some(w) = m.ws.get(&ws)
                && w.turns >= m.max_turns_per_ws
            {
                Some(format!("max_turns_per_workstream:{}", w.spec.slug))
            } else {
                None
            }
        };
        if let Some(which) = exhausted {
            let reason = PauseReason::BudgetExhausted { which };
            let mission_id = {
                let m = self.mission.as_mut().unwrap();
                m.paused = Some(reason.clone());
                m.pending.push_back(dispatch);
                m.plan.mission_id
            };
            self.shared
                .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                    mission: mission_id,
                    state: MissionState::Paused { reason },
                    detail: None,
                }));
            return;
        }
        self.dispatch_now(dispatch).await;
    }

    async fn dispatch_now(&mut self, dispatch: Dispatch) {
        match dispatch {
            Dispatch::Helm { ws, purpose, order } => self.dispatch_helm(ws, purpose, order).await,
            Dispatch::Kobayashi { ws, round } => self.dispatch_kobayashi(ws, round).await,
            Dispatch::Comms => self.dispatch_comms().await,
        }
    }

    async fn dispatch_helm(&mut self, ws: WorkstreamId, purpose: TurnPurpose, order: Order) {
        // Provision on first use.
        let needs_provision = self
            .mission
            .as_ref()
            .and_then(|m| m.ws.get(&ws))
            .is_some_and(|w| w.provisioned.is_none());
        if needs_provision {
            let (mission_slug, spec) = {
                let m = self.mission.as_ref().unwrap();
                (m.plan.slug.clone(), m.ws[&ws].spec.clone())
            };
            let cfg = self.shared.config.claude.clone();
            let helper = self.shared.helper_path.clone();
            let result = self
                .shared
                .blocking(move |d| {
                    engineering::provision(
                        d,
                        d,
                        &cfg,
                        &helper,
                        &mission_slug,
                        spec.id,
                        &spec.slug,
                        &spec.base_ref,
                        None,
                    )
                })
                .await;
            match result {
                Ok(p) => {
                    let path = p.handle.path.clone();
                    if let Some(m) = self.mission.as_mut()
                        && let Some(w) = m.ws.get_mut(&ws)
                    {
                        w.provisioned = Some(p);
                    }
                    self.shared.emit(BridgeEvent::WorkstreamProvisioned {
                        id: ws,
                        worktree_path: path,
                    });
                }
                Err(e) => {
                    self.set_status(
                        ws,
                        WorkstreamStatus::Failed {
                            reason: format!("provisioning failed: {e}"),
                        },
                    );
                    self.maybe_finish();
                    return;
                }
            }
        }
        if purpose == TurnPurpose::Work {
            self.set_status(ws, WorkstreamStatus::Working);
        }
        let (cwd, resume) = {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            let Some(w) = m.ws.get_mut(&ws) else { return };
            w.busy = true;
            (
                w.provisioned
                    .as_ref()
                    .expect("provisioned above")
                    .handle
                    .path
                    .clone(),
                w.helm_session.clone(),
            )
        };
        let profile = station_profile(Station::Helm, &self.shared.config);
        let order_id = order.id;
        let inv = ClaudeInvocation {
            prompt: order.prompt,
            cwd,
            resume,
            max_turns: order.max_turns,
            allowed_tools: order.allowed_tools,
            disallowed_tools: order.disallowed_tools,
            model: order.model.or(Some(profile.model.clone())),
            json_schema: None,
            mcp_config: None,
            append_system_prompt: Some(profile.append_system_prompt.clone()),
            output_format: OutputFormat::StreamJson,
            setting_sources: vec!["project".into(), "local".into()],
        };
        let ctx = TurnCtx {
            workstream: ws,
            station: Station::Helm,
            kobayashi: false,
            pid_register: None,
        };
        let started_at = Utc::now();
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let result = deps.run_turn(inv, ctx).await;
            let _ = tx
                .send(Internal::TurnDone {
                    ws,
                    purpose,
                    order_id,
                    started_at,
                    result,
                })
                .await;
        });
    }

    async fn dispatch_kobayashi(&mut self, ws: WorkstreamId, round: u32) {
        let (spec, slug, merge_gate, impl_handle) = {
            let Some(m) = self.mission.as_ref() else {
                return;
            };
            let Some(w) = m.ws.get(&ws) else { return };
            let Some(handle) = w.provisioned.as_ref().map(|p| p.handle.clone()) else {
                // Cannot test what was never provisioned; fail safe.
                self.set_status(
                    ws,
                    WorkstreamStatus::Failed {
                        reason: "no provisioned worktree to test".into(),
                    },
                );
                return;
            };
            (w.spec.clone(), m.plan.slug.clone(), w.merge_gate, handle)
        };
        if merge_gate {
            // Post-conflict testing counts as the checks phase of the queue.
            let m = self.mission.as_mut().unwrap();
            if m.queue.state_of(ws) == Some(MergeQueueState::ConflictFix) {
                m.queue.set_state(ws, MergeQueueState::ChecksRunning);
                self.emit_queue_update();
            }
        }
        self.set_status(ws, WorkstreamStatus::UnderTest { round });
        if let Some(m) = self.mission.as_mut()
            && let Some(w) = m.ws.get_mut(&ws)
        {
            w.busy = true;
        }
        let cfg = self.shared.config.claude.clone();
        let helper = self.shared.helper_path.clone();
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let result = KobayashiRunner::run_round(
                &*deps,
                &cfg,
                &helper,
                &spec,
                &slug,
                &impl_handle,
                round,
            )
            .await;
            let _ = tx.send(Internal::KobayashiDone { ws, round, result }).await;
        });
    }

    async fn dispatch_comms(&mut self) {
        let (plan, summary, captain_ws) = {
            let Some(m) = self.mission.as_ref() else {
                return;
            };
            let summary = m
                .order
                .iter()
                .map(|id| {
                    let w = &m.ws[id];
                    format!("{}: {:?}", w.spec.slug, w.status)
                })
                .collect::<Vec<_>>()
                .join("\n");
            (m.plan.clone(), summary, m.captain_ws)
        };
        let profile = station_profile(Station::Comms, &self.shared.config);
        let order_id = OrderId::new();
        let inv = ClaudeInvocation {
            prompt: prompts::comms_mission_report(&plan, &summary),
            cwd: self.shared.deps.repo_root(),
            resume: None,
            max_turns: profile.max_turns_default,
            allowed_tools: profile.allowed_tools.clone(),
            disallowed_tools: profile.disallowed_tools.clone(),
            model: Some(profile.model.clone()),
            json_schema: None,
            mcp_config: None,
            append_system_prompt: Some(profile.append_system_prompt.clone()),
            output_format: OutputFormat::StreamJson,
            setting_sources: Vec::new(),
        };
        let ctx = TurnCtx {
            workstream: captain_ws,
            station: Station::Comms,
            kobayashi: false,
            pid_register: None,
        };
        let started_at = Utc::now();
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let result = deps.run_turn(inv, ctx).await;
            let _ = tx
                .send(Internal::CommsDone {
                    order_id,
                    started_at,
                    result,
                })
                .await;
        });
    }

    // -- turn results -------------------------------------------------------

    async fn turn_done(
        &mut self,
        ws: WorkstreamId,
        purpose: TurnPurpose,
        order_id: OrderId,
        started_at: DateTime<Utc>,
        result: Result<TurnOutcome, EngineError>,
    ) {
        if let Some(m) = self.mission.as_mut()
            && let Some(w) = m.ws.get_mut(&ws)
        {
            w.busy = false;
        }
        let outcome = match result {
            Ok(o) => o,
            Err(e) => {
                self.set_status(
                    ws,
                    WorkstreamStatus::Failed {
                        reason: format!("helm turn failed: {e}"),
                    },
                );
                self.abandon_queue_entry(ws).await;
                self.start_eligible().await;
                return;
            }
        };
        self.record_turn_outcome(ws, Station::Helm, order_id, started_at, &outcome);

        // Session continuity: remember the session for --resume from the
        // same worktree.
        if let Some(sid) = outcome.result.as_ref().and_then(|r| r.session_id.clone()) {
            let session = SessionId(sid);
            let cwd = self
                .mission
                .as_ref()
                .and_then(|m| m.ws.get(&ws))
                .and_then(|w| w.provisioned.as_ref())
                .map(|p| p.handle.path.clone());
            if let Some(m) = self.mission.as_mut()
                && let Some(w) = m.ws.get_mut(&ws)
            {
                w.helm_session = Some(session.clone());
            }
            if let Some(cwd) = cwd
                && let Err(e) = self
                    .shared
                    .deps
                    .record_session(ws, Station::Helm, &session, &cwd)
            {
                tracing::warn!("failed to record session: {e}");
            }
        }

        // React to throttling BEFORE dispatching anything further.
        if let Some(hit) = outcome.rate_limit.clone() {
            self.pause_rate_limited(hit);
        }

        let success = outcome.exit == ExitClass::Success
            && outcome.result.as_ref().map(|r| !r.is_error).unwrap_or(true);
        if !success {
            let subtype = outcome
                .result
                .as_ref()
                .map(|r| r.subtype.clone())
                .unwrap_or_else(|| format!("{:?}", outcome.exit));
            self.set_status(
                ws,
                WorkstreamStatus::Failed {
                    reason: format!("helm turn ended with {subtype}"),
                },
            );
            self.abandon_queue_entry(ws).await;
            self.start_eligible().await;
            return;
        }

        match purpose {
            TurnPurpose::Work | TurnPurpose::FixBreach => {
                let (round, merge_gate, pending_rebase) = {
                    let m = self.mission.as_ref().unwrap();
                    let w = &m.ws[&ws];
                    (w.rounds + 1, w.merge_gate, w.pending_rebase)
                };
                if merge_gate {
                    let m = self.mission.as_mut().unwrap();
                    if m.queue.state_of(ws) == Some(MergeQueueState::ConflictFix) {
                        m.queue.set_state(ws, MergeQueueState::ChecksRunning);
                        self.emit_queue_update();
                    }
                    self.try_dispatch(Dispatch::Kobayashi { ws, round }).await;
                } else if pending_rebase {
                    // Quiet point: rebase onto the new main before testing.
                    if let Some(m) = self.mission.as_mut()
                        && let Some(w) = m.ws.get_mut(&ws)
                    {
                        w.pending_rebase = false;
                        w.stashed = Some(Dispatch::Kobayashi { ws, round });
                    }
                    self.spawn_quiet_rebase(ws);
                } else {
                    self.try_dispatch(Dispatch::Kobayashi { ws, round }).await;
                }
            }
            TurnPurpose::FixConflicts { in_queue: true } => {
                let round = {
                    let m = self.mission.as_ref().unwrap();
                    m.ws[&ws].rounds + 1
                };
                // Fresh Kobayashi round before confirmation: post-conflict
                // code is new code.
                self.try_dispatch(Dispatch::Kobayashi { ws, round }).await;
            }
            TurnPurpose::FixConflicts { in_queue: false } => {
                let stashed = self
                    .mission
                    .as_mut()
                    .and_then(|m| m.ws.get_mut(&ws))
                    .and_then(|w| w.stashed.take());
                match stashed {
                    Some(d) => self.try_dispatch(d).await,
                    None => self.set_status(ws, WorkstreamStatus::Working),
                }
            }
        }
    }

    async fn kobayashi_done(
        &mut self,
        ws: WorkstreamId,
        round: u32,
        result: Result<bridge_core::BattleReport, KobayashiError>,
    ) {
        if let Some(m) = self.mission.as_mut() {
            if let Some(w) = m.ws.get_mut(&ws) {
                w.busy = false;
                w.rounds = round;
                w.turns += 1;
            }
            // A Kobayashi round consumes a turn of budget.
            m.total_turns += 1;
        }
        self.emit_budget();
        let report = match result {
            Ok(r) => r,
            Err(e) => {
                self.set_status(
                    ws,
                    WorkstreamStatus::Failed {
                        reason: format!("kobayashi round failed: {e}"),
                    },
                );
                self.abandon_queue_entry(ws).await;
                self.start_eligible().await;
                return;
            }
        };
        self.shared
            .emit(BridgeEvent::BattleReportFiled(report.clone()));
        let merge_gate = self
            .mission
            .as_ref()
            .and_then(|m| m.ws.get(&ws))
            .is_some_and(|w| w.merge_gate);
        match report.verdict {
            bridge_core::Verdict::Clean => {
                if merge_gate {
                    {
                        let m = self.mission.as_mut().unwrap();
                        m.queue.set_state(ws, MergeQueueState::AwaitingConfirmation);
                    }
                    self.set_status(ws, WorkstreamStatus::InMergeQueue);
                    self.emit_queue_update();
                    self.request_merge_confirmation(ws, None, None).await;
                } else {
                    self.set_status(ws, WorkstreamStatus::ReadyToMerge);
                    {
                        let m = self.mission.as_mut().unwrap();
                        m.seq += 1;
                        let seq = m.seq;
                        let branch = m.ws[&ws].spec.branch_name(&m.plan.slug);
                        m.queue.enqueue(ws, seq, branch);
                    }
                    self.set_status(ws, WorkstreamStatus::InMergeQueue);
                    self.emit_queue_update();
                    self.pump_queue().await;
                }
            }
            bridge_core::Verdict::Breached => {
                let max_rounds = self.shared.config.budgets.max_kobayashi_rounds;
                if round >= max_rounds {
                    // Stayed breached: merge only via explicit user override.
                    // The mission deliberately does NOT auto-complete here: a
                    // flagged workstream awaits an explicit user decision
                    // (OverrideFlagged to merge anyway, or WindDown to settle).
                    // The GUI surfaces this; unattended drivers act on the
                    // Flagged event themselves (see the headless driver).
                    self.set_status(ws, WorkstreamStatus::Flagged);
                    if merge_gate {
                        self.abandon_queue_entry(ws).await;
                    }
                } else {
                    if merge_gate {
                        let m = self.mission.as_mut().unwrap();
                        if m.queue.state_of(ws) == Some(MergeQueueState::ChecksRunning) {
                            m.queue.set_state(ws, MergeQueueState::ConflictFix);
                            self.emit_queue_update();
                        }
                    }
                    self.set_status(ws, WorkstreamStatus::Breached { round });
                    let spec = self.mission.as_ref().unwrap().ws[&ws].spec.clone();
                    let profile = station_profile(Station::Helm, &self.shared.config);
                    let mut order = Order::new(
                        ws,
                        Station::Helm,
                        prompts::helm_fix(&spec, &report.findings),
                    );
                    order.max_turns = profile.max_turns_default;
                    order.allowed_tools = profile.allowed_tools.clone();
                    order.disallowed_tools = profile.disallowed_tools.clone();
                    self.screen_and_dispatch(
                        Dispatch::Helm {
                            ws,
                            purpose: TurnPurpose::FixBreach,
                            order,
                        },
                        &profile.allowed_tools,
                    )
                    .await;
                }
            }
        }
    }

    async fn comms_done(
        &mut self,
        order_id: OrderId,
        started_at: DateTime<Utc>,
        result: Result<TurnOutcome, EngineError>,
    ) {
        let Some(m) = self.mission.as_ref() else {
            return;
        };
        let mission_id = m.plan.mission_id;
        let captain_ws = m.captain_ws;
        match result {
            Ok(outcome) => {
                self.record_turn_outcome(captain_ws, Station::Comms, order_id, started_at, &outcome)
            }
            Err(e) => tracing::warn!("comms report turn failed: {e}"),
        }
        if let Err(e) = self.shared.deps.record_mission_complete(mission_id) {
            tracing::warn!("failed to record mission completion: {e}");
        }
        // Honest final state: failed workstreams fail the mission; flagged
        // ones (never merged) complete it with a warning detail.
        let (failed, flagged) = {
            let m = self.mission.as_ref().expect("mission checked above");
            let with_status = |pred: fn(&WorkstreamStatus) -> bool| {
                m.order
                    .iter()
                    .filter(|id| pred(&m.ws[id].status))
                    .map(|id| m.ws[id].spec.slug.clone())
                    .collect::<Vec<_>>()
            };
            (
                with_status(|s| matches!(s, WorkstreamStatus::Failed { .. })),
                with_status(|s| matches!(s, WorkstreamStatus::Flagged)),
            )
        };
        let (state, detail) = if failed.is_empty() {
            let detail = (!flagged.is_empty())
                .then(|| format!("flagged (unmerged) workstreams: {}", flagged.join(", ")));
            (MissionState::Complete, detail)
        } else {
            (
                MissionState::Failed {
                    reason: format!("workstreams failed: {}", failed.join(", ")),
                },
                None,
            )
        };
        self.shared
            .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                mission: mission_id,
                state,
                detail,
            }));
        self.exit = Some(Ok(()));
    }

    // -- merge queue ----------------------------------------------------------

    async fn pump_queue(&mut self) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        if m.queue.busy() {
            return;
        }
        let Some(ws) = m.queue.next_candidate() else {
            return;
        };
        let Some(handle) = m.ws[&ws].provisioned.as_ref().map(|p| p.handle.clone()) else {
            m.queue.remove(ws);
            self.set_status(
                ws,
                WorkstreamStatus::Failed {
                    reason: "no provisioned worktree to merge".into(),
                },
            );
            self.emit_queue_update();
            return;
        };
        m.queue.set_state(ws, MergeQueueState::Rebasing);
        self.set_status(ws, WorkstreamStatus::Rebasing);
        self.emit_queue_update();
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let branch = handle.branch.clone();
            let result = tokio::task::spawn_blocking(move || -> Result<RebaseData, GitError> {
                let target = deps.main_branch()?;
                let outcome = deps.rebase_onto(&handle, &target)?;
                let diff_stat = if matches!(outcome, RebaseOutcome::Clean) {
                    Some(deps.diff_stat_against_main(&branch)?)
                } else {
                    None
                };
                Ok(RebaseData {
                    outcome,
                    diff_stat,
                    target,
                })
            })
            .await
            .expect("rebase task panicked");
            let _ = tx
                .send(Internal::RebaseDone {
                    ws,
                    in_queue: true,
                    result,
                })
                .await;
        });
    }

    /// Quiet-point rebase of an active workstream after main moved.
    fn spawn_quiet_rebase(&mut self, ws: WorkstreamId) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        let Some(w) = m.ws.get_mut(&ws) else { return };
        let Some(handle) = w.provisioned.as_ref().map(|p| p.handle.clone()) else {
            return;
        };
        w.busy = true;
        self.set_status(ws, WorkstreamStatus::Rebasing);
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let result = tokio::task::spawn_blocking(move || -> Result<RebaseData, GitError> {
                let target = deps.main_branch()?;
                let outcome = deps.rebase_onto(&handle, &target)?;
                Ok(RebaseData {
                    outcome,
                    diff_stat: None,
                    target,
                })
            })
            .await
            .expect("rebase task panicked");
            let _ = tx
                .send(Internal::RebaseDone {
                    ws,
                    in_queue: false,
                    result,
                })
                .await;
        });
    }

    async fn rebase_done(
        &mut self,
        ws: WorkstreamId,
        in_queue: bool,
        result: Result<RebaseData, GitError>,
    ) {
        if !in_queue
            && let Some(m) = self.mission.as_mut()
            && let Some(w) = m.ws.get_mut(&ws)
        {
            w.busy = false;
        }
        match result {
            Ok(RebaseData {
                outcome: RebaseOutcome::Clean,
                diff_stat,
                target,
            }) => {
                if in_queue {
                    {
                        let m = self.mission.as_mut().unwrap();
                        m.queue.set_state(ws, MergeQueueState::AwaitingConfirmation);
                    }
                    self.set_status(ws, WorkstreamStatus::InMergeQueue);
                    self.emit_queue_update();
                    self.request_merge_confirmation(ws, diff_stat, Some(target))
                        .await;
                } else {
                    let stashed = self
                        .mission
                        .as_mut()
                        .and_then(|m| m.ws.get_mut(&ws))
                        .and_then(|w| w.stashed.take());
                    match stashed {
                        Some(d) => self.try_dispatch(d).await,
                        None => self.set_status(ws, WorkstreamStatus::Working),
                    }
                }
            }
            Ok(RebaseData {
                outcome: RebaseOutcome::Conflicts { files },
                ..
            }) => {
                if in_queue {
                    let m = self.mission.as_mut().unwrap();
                    m.queue.set_state(ws, MergeQueueState::ConflictFix);
                    if let Some(w) = m.ws.get_mut(&ws) {
                        w.merge_gate = true;
                    }
                    self.emit_queue_update();
                }
                self.set_status(ws, WorkstreamStatus::ConflictFix);
                let spec = self.mission.as_ref().unwrap().ws[&ws].spec.clone();
                let files: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
                let profile = station_profile(Station::Helm, &self.shared.config);
                let mut order = Order::new(
                    ws,
                    Station::Helm,
                    prompts::helm_resolve_conflicts(&spec, &files),
                );
                order.max_turns = profile.max_turns_default;
                order.allowed_tools = profile.allowed_tools.clone();
                order.disallowed_tools = profile.disallowed_tools.clone();
                self.screen_and_dispatch(
                    Dispatch::Helm {
                        ws,
                        purpose: TurnPurpose::FixConflicts { in_queue },
                        order,
                    },
                    &profile.allowed_tools,
                )
                .await;
            }
            Err(e) => {
                self.set_status(
                    ws,
                    WorkstreamStatus::Failed {
                        reason: format!("rebase failed: {e}"),
                    },
                );
                self.abandon_queue_entry(ws).await;
                self.start_eligible().await;
            }
        }
    }

    async fn request_merge_confirmation(
        &mut self,
        ws: WorkstreamId,
        diff_stat: Option<String>,
        target: Option<String>,
    ) {
        let (branch, summary) = {
            let Some(m) = self.mission.as_ref() else {
                return;
            };
            let w = &m.ws[&ws];
            (w.spec.branch_name(&m.plan.slug), w.spec.title.clone())
        };
        let target = match target {
            Some(t) => t,
            None => self
                .shared
                .blocking(|d| d.main_branch())
                .await
                .unwrap_or_else(|_| "main".into()),
        };
        let diff_stat = match diff_stat {
            Some(d) => d,
            None => {
                let b = branch.clone();
                self.shared
                    .blocking(move |d| d.diff_stat_against_main(&b))
                    .await
                    .unwrap_or_default()
            }
        };
        let proposal = MergeProposal {
            workstream: ws,
            branch,
            target,
            summary,
            diff_stat,
        };
        if let Some(m) = self.mission.as_mut() {
            m.pending_merges.insert(ws, proposal.clone());
        }
        self.shared
            .emit(BridgeEvent::MergeConfirmationRequested(proposal));
    }

    async fn confirm_merge(&mut self, ws: WorkstreamId, approved: bool) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        if m.queue.state_of(ws) != Some(MergeQueueState::AwaitingConfirmation) {
            tracing::warn!("ConfirmMerge for {ws} ignored: not awaiting confirmation");
            return;
        }
        m.pending_merges.remove(&ws);
        if !approved {
            self.set_status(
                ws,
                WorkstreamStatus::Failed {
                    reason: "merge rejected by user".into(),
                },
            );
            self.abandon_queue_entry(ws).await;
            self.start_eligible().await;
            return;
        }
        m.queue.set_state(ws, MergeQueueState::Merging);
        let branch = m.ws[&ws].spec.branch_name(&m.plan.slug);
        let mode = self.shared.config.merge.mode;
        self.emit_queue_update();
        let deps = self.shared.deps.clone();
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            let result = tokio::task::spawn_blocking(move || deps.merge_into_main(&branch, mode))
                .await
                .expect("merge task panicked");
            let _ = tx.send(Internal::MergeDone { ws, result }).await;
        });
    }

    async fn merge_done(&mut self, ws: WorkstreamId, result: Result<MergeOutcome, GitError>) {
        match result {
            Ok(_) => {
                {
                    let Some(m) = self.mission.as_mut() else {
                        return;
                    };
                    m.queue.set_state(ws, MergeQueueState::Done);
                    m.queue.remove(ws);
                    if let Some(w) = m.ws.get_mut(&ws) {
                        w.merge_gate = false;
                    }
                }
                self.set_status(ws, WorkstreamStatus::Merged);
                self.emit_queue_update();
                // Post-merge: every still-active workstream not in the queue
                // gets rebased onto the new main at its next quiet point.
                let (idle, busy): (Vec<WorkstreamId>, Vec<WorkstreamId>) = {
                    let m = self.mission.as_ref().unwrap();
                    let active: Vec<(WorkstreamId, bool)> =
                        m.ws.iter()
                            .filter(|(id, w)| {
                                **id != ws
                                    && w.started
                                    && w.provisioned.is_some()
                                    && !matches!(
                                        w.status,
                                        WorkstreamStatus::Merged
                                            | WorkstreamStatus::Failed { .. }
                                            | WorkstreamStatus::Flagged
                                    )
                                    && m.queue.state_of(**id).is_none()
                            })
                            .map(|(id, w)| (*id, w.busy))
                            .collect();
                    (
                        active
                            .iter()
                            .filter(|(_, b)| !b)
                            .map(|(id, _)| *id)
                            .collect(),
                        active
                            .iter()
                            .filter(|(_, b)| *b)
                            .map(|(id, _)| *id)
                            .collect(),
                    )
                };
                for id in busy {
                    if let Some(m) = self.mission.as_mut()
                        && let Some(w) = m.ws.get_mut(&id)
                    {
                        w.pending_rebase = true;
                    }
                }
                for id in idle {
                    self.spawn_quiet_rebase(id);
                }
                // Dependents whose dependencies just merged may now start.
                self.start_eligible().await;
                self.pump_queue().await;
                self.maybe_finish();
            }
            Err(e) => {
                self.set_status(
                    ws,
                    WorkstreamStatus::Failed {
                        reason: format!("merge failed: {e}"),
                    },
                );
                self.abandon_queue_entry(ws).await;
                self.start_eligible().await;
            }
        }
    }

    /// Drop a workstream's queue entry (failure/rejection paths) and let
    /// the queue move on.
    async fn abandon_queue_entry(&mut self, ws: WorkstreamId) {
        let removed = {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            m.pending_merges.remove(&ws);
            if m.queue.contains(ws) {
                m.queue.remove(ws);
                if let Some(w) = m.ws.get_mut(&ws) {
                    w.merge_gate = false;
                }
                true
            } else {
                false
            }
        };
        if removed {
            self.emit_queue_update();
            self.pump_queue().await;
        }
    }

    async fn override_flagged(&mut self, ws: WorkstreamId) {
        {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            let Some(w) = m.ws.get(&ws) else { return };
            if w.status != WorkstreamStatus::Flagged {
                tracing::warn!("OverrideFlagged for {ws} ignored: not flagged");
                return;
            }
            m.seq += 1;
            let seq = m.seq;
            let branch = m.ws[&ws].spec.branch_name(&m.plan.slug);
            m.queue.enqueue(ws, seq, branch);
        }
        self.set_status(ws, WorkstreamStatus::InMergeQueue);
        self.emit_queue_update();
        self.pump_queue().await;
    }

    // -- pauses ------------------------------------------------------------

    fn pause_rate_limited(&mut self, hit: RateLimitHit) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        if m.paused.is_some() {
            return;
        }
        let reason = PauseReason::RateLimited {
            retry_at: hit.retry_at,
        };
        m.paused = Some(reason.clone());
        let mission_id = m.plan.mission_id;
        self.shared
            .emit(BridgeEvent::RateLimit(RateLimitState::Hit {
                retry_at: hit.retry_at,
            }));
        self.shared
            .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                mission: mission_id,
                state: MissionState::Paused { reason },
                detail: Some(hit.trigger.clone()),
            }));
        let delay = hit
            .retry_at
            .and_then(|t| (t - Utc::now()).to_std().ok())
            .unwrap_or(DEFAULT_RATE_LIMIT_PROBE);
        let tx = self.shared.internal_tx.clone();
        self.shared.spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Internal::RateLimitProbe).await;
        });
    }

    async fn rate_limit_probe(&mut self) {
        let mission_id = {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            if !matches!(m.paused, Some(PauseReason::RateLimited { .. })) {
                return;
            }
            m.paused = None;
            m.plan.mission_id
        };
        self.shared
            .emit(BridgeEvent::RateLimit(RateLimitState::Cleared));
        self.shared
            .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                mission: mission_id,
                state: MissionState::Executing,
                detail: None,
            }));
        self.flush_pending().await;
    }

    async fn extend_budget(&mut self, ext: BudgetExtension) {
        let resumed = {
            let Some(m) = self.mission.as_mut() else {
                return;
            };
            m.max_total_turns += ext.extra_total_turns;
            m.max_turns_per_ws += ext.extra_turns_per_workstream;
            if ext.extra_wall_clock_secs > 0 {
                m.max_wall_clock_secs =
                    Some(m.max_wall_clock_secs.unwrap_or(0) + ext.extra_wall_clock_secs);
            }
            if matches!(m.paused, Some(PauseReason::BudgetExhausted { .. })) {
                m.paused = None;
                Some(m.plan.mission_id)
            } else {
                None
            }
        };
        self.emit_budget();
        if let Some(mission_id) = resumed {
            self.shared
                .emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
                    mission: mission_id,
                    state: MissionState::Executing,
                    detail: None,
                }));
            self.flush_pending().await;
        }
    }

    async fn flush_pending(&mut self) {
        loop {
            let next = {
                let Some(m) = self.mission.as_mut() else {
                    return;
                };
                if m.paused.is_some() {
                    return;
                }
                m.pending.pop_front()
            };
            match next {
                Some(d) => self.try_dispatch(d).await,
                None => return,
            }
        }
    }

    /// Re-emit every currently-actionable one-shot signal. Called when a
    /// consumer detects it lagged the broadcast bus and may have dropped an
    /// `EscalationRequested` / `MergeConfirmationRequested`. Idempotent: the
    /// events are the same tickets/proposals still held in state.
    fn resync_actionable(&mut self) {
        // Hook escalations live on the control server's broker.
        for ticket in self.shared.deps.pending_hook_escalations() {
            self.shared.emit(BridgeEvent::EscalationRequested(ticket));
        }
        let (tickets, proposals) = {
            let Some(m) = self.mission.as_ref() else {
                return;
            };
            (
                m.escalations
                    .values()
                    .map(|(t, _)| t.clone())
                    .collect::<Vec<_>>(),
                m.pending_merges.values().cloned().collect::<Vec<_>>(),
            )
        };
        for ticket in tickets {
            self.shared.emit(BridgeEvent::EscalationRequested(ticket));
        }
        for proposal in proposals {
            self.shared
                .emit(BridgeEvent::MergeConfirmationRequested(proposal));
        }
    }

    async fn resolve_escalation(&mut self, id: EscalationId, decision: UserDecision) {
        let dispatch = {
            let Some(m) = self.mission.as_mut() else {
                // No mission context: it can only be a hook ticket.
                self.shared
                    .deps
                    .resolve_hook_escalation(id, decision.clone());
                self.shared
                    .emit(BridgeEvent::EscalationResolved { id, decision });
                return;
            };
            m.escalations
                .remove(&id)
                .map(|(_ticket, dispatch)| dispatch)
        };
        let Some(dispatch) = dispatch else {
            // Not a controller order-screening ticket: it belongs to the
            // control server's hook broker (a mid-run tool-call escalation).
            self.shared
                .deps
                .resolve_hook_escalation(id, decision.clone());
            self.shared
                .emit(BridgeEvent::EscalationResolved { id, decision });
            return;
        };
        self.shared.emit(BridgeEvent::EscalationResolved {
            id,
            decision: decision.clone(),
        });
        match decision {
            UserDecision::Approve => self.try_dispatch(dispatch).await,
            UserDecision::Deny { reason } => {
                if let Some(ws) = dispatch.ws() {
                    self.set_status(
                        ws,
                        WorkstreamStatus::Failed {
                            reason: format!("order denied by user: {reason}"),
                        },
                    );
                }
                self.start_eligible().await;
            }
        }
    }

    // -- completion --------------------------------------------------------

    /// When nothing can make further progress, schedule the Comms report.
    /// Sync + deferred through the internal channel so any handler can call
    /// it without creating async recursion.
    fn maybe_finish(&mut self) {
        let finish = {
            let Some(m) = self.mission.as_ref() else {
                return;
            };
            if m.comms_dispatched {
                return;
            }
            let nothing_running = m.ws.values().all(|w| !w.busy)
                && m.pending.is_empty()
                && m.escalations.is_empty()
                && m.queue.is_empty()
                // The conference persists after launch (for a later
                // mid-mission amendment), so unlike the old mutually
                // exclusive `planning` field this only blocks completion
                // while an actual Captain turn is still in flight.
                && self
                    .conference
                    .as_ref()
                    .is_none_or(|c| c.turn_in_flight.is_none());
            // Flagged is deliberately NOT settled: the mission waits for the
            // user to OverrideFlagged or WindDown. winding_down collapses all
            // remaining states to settled so a wind-down always terminates.
            let all_settled = m.ws.values().all(|w| {
                matches!(
                    w.status,
                    WorkstreamStatus::Merged
                        | WorkstreamStatus::Failed { .. }
                        | WorkstreamStatus::Cancelled
                ) || self.winding_down
            });
            nothing_running && all_settled
        };
        if finish {
            if let Some(m) = self.mission.as_mut() {
                m.comms_dispatched = true;
            }
            // The final report deliberately bypasses the budget gate: a
            // finished mission always gets its Comms turn.
            if self
                .shared
                .internal_tx
                .try_send(Internal::DispatchComms)
                .is_err()
            {
                tracing::error!("internal channel full; comms report dropped");
            }
        }
    }

    // -- bookkeeping ------------------------------------------------------

    fn set_status(&mut self, id: WorkstreamId, status: WorkstreamStatus) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        let Some(w) = m.ws.get_mut(&id) else { return };
        if w.status == status {
            return;
        }
        w.status = status.clone();
        if let Err(e) = self.shared.deps.record_workstream_status(id, &status) {
            tracing::warn!("failed to record workstream status: {e}");
        }
        self.shared
            .emit(BridgeEvent::WorkstreamStatus { id, status });
    }

    fn record_turn_outcome(
        &mut self,
        ws: WorkstreamId,
        station: Station,
        order_id: OrderId,
        started_at: DateTime<Utc>,
        outcome: &TurnOutcome,
    ) {
        let Some(m) = self.mission.as_mut() else {
            return;
        };
        let r = outcome.result.as_ref();
        let record = bridge_core::TurnRecord {
            order: order_id,
            workstream: ws,
            station,
            session: r.and_then(|x| x.session_id.clone()).map(SessionId),
            started_at,
            duration_ms: r.and_then(|x| x.duration_ms).unwrap_or(0),
            num_turns: r.and_then(|x| x.num_turns).unwrap_or(0),
            is_error: r
                .map(|x| x.is_error)
                .unwrap_or(outcome.exit != ExitClass::Success),
            subtype: r
                .map(|x| x.subtype.clone())
                .unwrap_or_else(|| format!("{:?}", outcome.exit)),
            total_cost_usd: r.and_then(|x| x.total_cost_usd),
            input_tokens: r
                .and_then(|x| x.usage.as_ref())
                .and_then(|u| u.input_tokens),
            output_tokens: r
                .and_then(|x| x.usage.as_ref())
                .and_then(|u| u.output_tokens),
        };
        m.total_turns += 1;
        m.total_cost += record.total_cost_usd.unwrap_or(0.0);
        if let Some(w) = m.ws.get_mut(&ws) {
            w.turns += 1;
        }
        let mission_id = m.plan.mission_id;
        if let Err(e) = self.shared.deps.record_turn(mission_id, &record) {
            tracing::warn!("failed to record turn: {e}");
        }
        self.shared.emit(BridgeEvent::TurnCompleted(record));
        self.emit_budget();
    }

    /// Mirrors `record_turn_outcome`'s field mapping, but the conference has
    /// no `Mission` to early-return through: it records against the
    /// conference's mission id directly, and only touches mission-level
    /// budget accounting when a mission is actually running (mid-mission
    /// conference turns then count toward `max_total_turns`).
    fn record_conference_turn(
        &mut self,
        order_id: OrderId,
        started_at: DateTime<Utc>,
        outcome: &TurnOutcome,
    ) {
        let Some(conf) = self.conference.as_ref() else {
            return;
        };
        let mission_id = conf.mission_id;
        let ws = WorkstreamId(mission_id.0);
        let r = outcome.result.as_ref();
        let record = bridge_core::TurnRecord {
            order: order_id,
            workstream: ws,
            station: Station::Captain,
            session: r.and_then(|x| x.session_id.clone()).map(SessionId),
            started_at,
            duration_ms: r.and_then(|x| x.duration_ms).unwrap_or(0),
            num_turns: r.and_then(|x| x.num_turns).unwrap_or(0),
            is_error: r
                .map(|x| x.is_error)
                .unwrap_or(outcome.exit != ExitClass::Success),
            subtype: r
                .map(|x| x.subtype.clone())
                .unwrap_or_else(|| format!("{:?}", outcome.exit)),
            total_cost_usd: r.and_then(|x| x.total_cost_usd),
            input_tokens: r
                .and_then(|x| x.usage.as_ref())
                .and_then(|u| u.input_tokens),
            output_tokens: r
                .and_then(|x| x.usage.as_ref())
                .and_then(|u| u.output_tokens),
        };
        if let Some(m) = self.mission.as_mut() {
            m.total_turns += 1;
            m.total_cost += record.total_cost_usd.unwrap_or(0.0);
            if let Some(w) = m.ws.get_mut(&ws) {
                w.turns += 1;
            }
        }
        if let Err(e) = self.shared.deps.record_turn(mission_id, &record) {
            tracing::warn!("failed to record turn: {e}");
        }
        self.shared.emit(BridgeEvent::TurnCompleted(record));
        self.emit_budget();
    }

    fn emit_budget(&self) {
        let Some(m) = self.mission.as_ref() else {
            return;
        };
        let snapshot = BudgetSnapshot {
            mission: m.plan.mission_id,
            total_turns: m.total_turns,
            max_total_turns: m.max_total_turns,
            per_workstream: m
                .order
                .iter()
                .map(|id| WorkstreamTurns {
                    workstream: *id,
                    turns: m.ws[id].turns,
                })
                .collect(),
            wall_clock_secs: (Utc::now() - m.started_at).num_seconds().max(0) as u64,
            max_wall_clock_secs: m.max_wall_clock_secs,
            total_cost_usd: m.total_cost,
        };
        self.shared.emit(BridgeEvent::BudgetUpdate(snapshot));
    }

    fn emit_queue_update(&self) {
        if let Some(m) = self.mission.as_ref() {
            self.shared
                .emit(BridgeEvent::MergeQueueUpdate(m.queue.snapshot()));
        }
    }
}

#[cfg(test)]
mod tests;
