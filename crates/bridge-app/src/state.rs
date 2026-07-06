//! GUI state: a pure reducer over `BridgeEvent`, fully unit-testable
//! without egui.

use bridge_core::{
    BattleReport, BridgeEvent, BudgetSnapshot, EscalationId, EscalationTicket, HookDecisionRecord,
    LogEntry, MergeProposal, MergeQueueEntry, MissionState, PauseReason, PlanDiff, PlanDraft,
    RateLimitState, TurnRecord, WorkstreamId, WorkstreamStatus,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;

/// Everything the panels render. Bounded buffers: logs and tactical feed
/// keep the newest `MAX_FEED` entries (drop oldest).
#[derive(Debug, Default)]
pub struct AppState {
    pub mission_state: Option<MissionState>,
    pub mission_detail: Option<String>,
    pub workstreams: HashMap<WorkstreamId, WorkstreamPanel>,
    /// Workstream ordering: first-seen order.
    pub workstream_order: Vec<WorkstreamId>,
    pub logs: Vec<LogEntry>,
    pub tactical_feed: Vec<HookDecisionRecord>,
    pub escalations: Vec<EscalationTicket>,
    pub merge_queue: Vec<MergeQueueEntry>,
    pub pending_merges: Vec<MergeProposal>,
    pub budget: Option<BudgetSnapshot>,
    pub rate_limit: Option<RateLimitState>,
    pub compat_warning: Option<(String, String, String)>,
    pub battle_reports: Vec<BattleReport>,
    /// Transcript of the Captain conference: who said what, oldest first.
    pub captain_feed: Vec<(CaptainSpeaker, String)>,
    /// The most recent plan proposal awaiting approval, if any.
    pub latest_proposal: Option<ProposalCard>,
    /// Ephemeral widget state (text buffers, selection). Not event-driven.
    pub ui: UiInputs,
}

/// Who authored a line in the Captain conference transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptainSpeaker {
    You,
    Captain,
}

/// A pending plan proposal, shown to the user for approval.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalCard {
    pub revision: u64,
    pub plan: PlanDraft,
    pub diff: Option<PlanDiff>,
}

/// Text buffers and selection owned by the GUI between frames.
#[derive(Debug, Default)]
pub struct UiInputs {
    /// Mission objective input box content.
    pub objective: String,
    /// Workstream selected on the deck (opens the workstream detail window).
    pub selected: Option<WorkstreamId>,
    /// Per-ticket deny-reason text fields in the escalation modals.
    pub deny_reasons: HashMap<EscalationId, String>,
    /// Editor launched by "Open in VS Code" (from `config.ui.editor_command`,
    /// set once at startup). Empty falls back to "code".
    pub editor_command: String,
    /// Guardrails feed (Tactical window): when true, also show routine
    /// Allow decisions; otherwise only denials and escalations
    /// (exceptions-only).
    pub show_all_guardrails: bool,
    /// Deck's tactical console clicked: shows the Tactical interim window.
    pub open_tactical: bool,
    /// Deck's Kobayashi Maru door clicked: shows the Kobayashi interim window.
    pub open_kobayashi: bool,
    /// Deck's computer console clicked: shows the Ship's Computer interim
    /// window (Ship's Log content).
    pub open_computer: bool,
    /// Deck's viewscreen clicked: shows the Mission Status interim window
    /// (mission title/state, merge queue, budget, Wind down / Stop).
    pub open_mission_status: bool,
    /// Hailing the Captain on the deck clicked: shows the Captain interim
    /// window (conference transcript and the latest plan proposal card).
    pub open_captain: bool,
}

pub const MAX_FEED: usize = 2_000;

/// Per-workstream detail pane data.
#[derive(Debug, Default)]
pub struct WorkstreamPanel {
    pub status: Option<WorkstreamStatus>,
    /// Rolling agent output (station-tagged lines), bounded MAX_FEED.
    pub output: Vec<(bridge_core::Station, String)>,
    pub tool_calls: Vec<String>,
    pub turns: Vec<TurnRecord>,
    /// Worktree path once provisioning completes; enables "Open in editor".
    pub worktree_path: Option<PathBuf>,
}

impl AppState {
    /// Apply one event. Total: every variant updates state; unknown data
    /// never panics. EscalationResolved removes its ticket; RateLimit
    /// Cleared clears the banner; MergeQueueUpdate replaces the queue
    /// snapshot; ConfirmMerge handling removes the proposal (done by the
    /// caller emitting fresh events, not here).
    pub fn apply(&mut self, event: BridgeEvent) {
        match event {
            BridgeEvent::Log(entry) => push_bounded(&mut self.logs, entry),
            BridgeEvent::MissionStatus(update) => {
                if update.state == MissionState::Executing {
                    self.latest_proposal = None;
                }
                self.mission_state = Some(update.state);
                self.mission_detail = update.detail;
            }
            BridgeEvent::WorkstreamStatus { id, status } => {
                if matches!(
                    status,
                    WorkstreamStatus::Merged
                        | WorkstreamStatus::Failed { .. }
                        | WorkstreamStatus::Cancelled
                ) {
                    self.pending_merges.retain(|p| p.workstream != id);
                }
                self.panel_mut(id).status = Some(status);
            }
            BridgeEvent::AgentOutput {
                workstream,
                station,
                text,
            } => push_bounded(&mut self.panel_mut(workstream).output, (station, text)),
            BridgeEvent::ToolCall {
                workstream,
                station,
                tool_name,
                summary,
            } => push_bounded(
                &mut self.panel_mut(workstream).tool_calls,
                format!("[{station}] {tool_name}: {summary}"),
            ),
            BridgeEvent::TurnCompleted(turn) => {
                push_bounded(&mut self.panel_mut(turn.workstream).turns, turn);
            }
            BridgeEvent::HookDecision(record) => push_bounded(&mut self.tactical_feed, record),
            BridgeEvent::EscalationRequested(ticket) => {
                if !self.escalations.iter().any(|t| t.id == ticket.id) {
                    self.escalations.push(ticket);
                }
            }
            BridgeEvent::EscalationResolved { id, decision: _ } => {
                self.escalations.retain(|t| t.id != id);
                self.ui.deny_reasons.remove(&id);
            }
            BridgeEvent::RateLimit(state) => {
                self.rate_limit = match state {
                    RateLimitState::Cleared => None,
                    hit @ RateLimitState::Hit { .. } => Some(hit),
                };
            }
            BridgeEvent::BudgetUpdate(snapshot) => self.budget = Some(snapshot),
            BridgeEvent::MergeQueueUpdate(entries) => self.merge_queue = entries,
            BridgeEvent::MergeConfirmationRequested(proposal) => {
                match self
                    .pending_merges
                    .iter_mut()
                    .find(|p| p.workstream == proposal.workstream)
                {
                    Some(existing) => *existing = proposal,
                    None => self.pending_merges.push(proposal),
                }
            }
            BridgeEvent::BattleReportFiled(report) => self.battle_reports.push(report),
            BridgeEvent::CompatWarning {
                detected,
                tested_min,
                tested_max,
            } => self.compat_warning = Some((detected, tested_min, tested_max)),
            BridgeEvent::WorkstreamProvisioned { id, worktree_path } => {
                self.panel_mut(id).worktree_path = Some(worktree_path);
            }
            BridgeEvent::UserSaid { mission: _, text } => {
                push_bounded(&mut self.captain_feed, (CaptainSpeaker::You, text));
            }
            BridgeEvent::CaptainSays { mission: _, text } => {
                push_bounded(&mut self.captain_feed, (CaptainSpeaker::Captain, text));
            }
            BridgeEvent::PlanProposed {
                mission: _,
                revision,
                plan,
                diff,
            } => {
                self.latest_proposal = Some(ProposalCard {
                    revision,
                    plan,
                    diff,
                });
            }
            BridgeEvent::ProposalRejected {
                mission: _,
                revision: _,
                reason,
            } => {
                push_bounded(
                    &mut self.captain_feed,
                    (
                        CaptainSpeaker::Captain,
                        format!("Proposal rejected: {reason}"),
                    ),
                );
            }
        }
    }

    /// Panel for a workstream, created (and appended to the first-seen
    /// workstream order) on first reference.
    fn panel_mut(&mut self, id: WorkstreamId) -> &mut WorkstreamPanel {
        if !self.workstreams.contains_key(&id) {
            self.workstream_order.push(id);
        }
        self.workstreams.entry(id).or_default()
    }

    /// Remove a merge proposal locally after the user clicked confirm or
    /// reject; the authoritative removal arrives via fresh events.
    pub fn remove_merge_proposal(&mut self, workstream: WorkstreamId) {
        self.pending_merges.retain(|p| p.workstream != workstream);
    }

    /// Remove an escalation ticket locally after the user answered it; the
    /// authoritative removal arrives via EscalationResolved.
    pub fn remove_escalation(&mut self, id: EscalationId) {
        self.escalations.retain(|t| t.id != id);
        self.ui.deny_reasons.remove(&id);
    }

    /// Banner text while rate limited; None when no banner should show.
    pub fn rate_limit_countdown_text(&self, now: DateTime<Utc>) -> Option<String> {
        match &self.rate_limit {
            Some(RateLimitState::Hit { retry_at: Some(at) }) => {
                let secs = (*at - now).num_seconds();
                if secs > 0 {
                    Some(format!(
                        "Rate limited - retrying in {}",
                        format_duration_secs(secs as u64)
                    ))
                } else {
                    Some("Rate limited - retrying now".to_owned())
                }
            }
            Some(RateLimitState::Hit { retry_at: None }) => {
                Some("Rate limited - waiting for reset".to_owned())
            }
            Some(RateLimitState::Cleared) | None => None,
        }
    }
}

/// Append, then trim from the FRONT so the newest `MAX_FEED` items remain.
fn push_bounded<T>(buf: &mut Vec<T>, item: T) {
    buf.push(item);
    if buf.len() > MAX_FEED {
        let excess = buf.len() - MAX_FEED;
        buf.drain(..excess);
    }
}

/// Short badge text for a workstream status.
pub fn status_label(status: &WorkstreamStatus) -> String {
    match status {
        WorkstreamStatus::Pending => "Pending".to_owned(),
        WorkstreamStatus::Working => "Working".to_owned(),
        WorkstreamStatus::UnderTest { round } => format!("Under test - round {round}"),
        WorkstreamStatus::Breached { round } => format!("Breached - round {round}"),
        WorkstreamStatus::ReadyToMerge => "Ready".to_owned(),
        WorkstreamStatus::Rebasing => "Rebasing".to_owned(),
        WorkstreamStatus::ConflictFix => "Conflict fix".to_owned(),
        WorkstreamStatus::InMergeQueue => "In queue".to_owned(),
        WorkstreamStatus::Merged => "Merged".to_owned(),
        WorkstreamStatus::Failed { .. } => "Failed".to_owned(),
        WorkstreamStatus::Flagged => "Flagged".to_owned(),
        WorkstreamStatus::Cancelled => "Cancelled".to_owned(),
    }
}

/// Header chip text for the mission state.
pub fn mission_state_label(state: &MissionState) -> String {
    match state {
        MissionState::Planning => "Planning".to_owned(),
        MissionState::Executing => "Executing".to_owned(),
        MissionState::Paused { reason } => format!("Paused - {}", pause_reason_text(reason)),
        MissionState::WindingDown => "Winding down".to_owned(),
        MissionState::Complete => "Complete".to_owned(),
        MissionState::Failed { reason } => format!("Failed - {reason}"),
    }
}

fn pause_reason_text(reason: &PauseReason) -> String {
    match reason {
        PauseReason::RateLimited { retry_at: Some(at) } => {
            format!("rate limited, retry at {}", at.format("%H:%M:%S UTC"))
        }
        PauseReason::RateLimited { retry_at: None } => "rate limited".to_owned(),
        PauseReason::BudgetExhausted { which } => format!("budget exhausted ({which})"),
        PauseReason::UserRequested => "user requested".to_owned(),
    }
}

/// Seconds until an escalation fails closed, floored at zero.
pub fn escalation_remaining_secs(ticket: &EscalationTicket, now: DateTime<Utc>) -> i64 {
    (ticket.expires_at - now).num_seconds().max(0)
}

/// "45s", "3m 20s", "1h 02m".
pub fn format_duration_secs(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::{
        BridgeEvent, BudgetSnapshot, DecisionKind, DecisionSource, EscalationId, EscalationTicket,
        HookDecisionRecord, LogEntry, LogLevel, MergeProposal, MergeQueueEntry, MergeQueueState,
        MissionId, MissionState, MissionStatusUpdate, OrderId, PauseReason, PlanDraft,
        PlanDraftWorkstream, RateLimitState, Station, TurnRecord, UserDecision, Verdict,
        WorkstreamId, WorkstreamStatus,
    };
    use chrono::{Duration, TimeZone, Utc};

    fn log_event(msg: &str) -> BridgeEvent {
        BridgeEvent::Log(LogEntry::new(LogLevel::Info, msg))
    }

    fn hook_decision(ws: WorkstreamId, rule: &str) -> HookDecisionRecord {
        HookDecisionRecord {
            timestamp: Utc::now(),
            workstream: ws,
            hook_event: "PreToolUse".into(),
            tool_name: Some("Bash".into()),
            decision: DecisionKind::Allow,
            reason: None,
            rule: rule.into(),
            source: DecisionSource::Passthrough,
            latency_ms: 1,
        }
    }

    fn ticket(id: EscalationId, ws: WorkstreamId) -> EscalationTicket {
        EscalationTicket {
            id,
            workstream: ws,
            question: "allow push?".into(),
            tool_name: Some("Bash".into()),
            tool_input_summary: "git push".into(),
            requested_at: Utc::now(),
            expires_at: Utc::now() + Duration::seconds(540),
        }
    }

    fn turn(ws: WorkstreamId) -> TurnRecord {
        TurnRecord {
            order: OrderId::new(),
            workstream: ws,
            station: Station::Helm,
            session: None,
            started_at: Utc::now(),
            duration_ms: 100,
            num_turns: 1,
            is_error: false,
            subtype: "success".into(),
            total_cost_usd: Some(0.01),
            input_tokens: Some(10),
            output_tokens: Some(20),
        }
    }

    fn snapshot(total: u32, max: u32) -> BudgetSnapshot {
        BudgetSnapshot {
            mission: MissionId::new(),
            total_turns: total,
            max_total_turns: max,
            per_workstream: vec![],
            wall_clock_secs: 30,
            max_wall_clock_secs: Some(120),
            total_cost_usd: 1.25,
        }
    }

    fn proposal(ws: WorkstreamId, summary: &str) -> MergeProposal {
        MergeProposal {
            workstream: ws,
            branch: "bridge/m/ws".into(),
            target: "main".into(),
            summary: summary.into(),
            diff_stat: "1 file changed".into(),
        }
    }

    #[test]
    fn log_appends() {
        let mut s = AppState::default();
        s.apply(log_event("hello"));
        assert_eq!(s.logs.len(), 1);
        assert_eq!(s.logs[0].message, "hello");
    }

    #[test]
    fn logs_bounded_drop_oldest() {
        let mut s = AppState::default();
        for i in 0..(MAX_FEED + 5) {
            s.apply(log_event(&format!("m{i}")));
        }
        assert_eq!(s.logs.len(), MAX_FEED);
        assert_eq!(s.logs[0].message, "m5", "oldest entries dropped first");
        assert_eq!(s.logs.last().unwrap().message, format!("m{}", MAX_FEED + 4));
    }

    #[test]
    fn mission_status_sets_state_and_detail() {
        let mut s = AppState::default();
        s.apply(BridgeEvent::MissionStatus(MissionStatusUpdate {
            mission: MissionId::new(),
            state: MissionState::Executing,
            detail: Some("3 workstreams".into()),
        }));
        assert_eq!(s.mission_state, Some(MissionState::Executing));
        assert_eq!(s.mission_detail.as_deref(), Some("3 workstreams"));
        s.apply(BridgeEvent::MissionStatus(MissionStatusUpdate {
            mission: MissionId::new(),
            state: MissionState::Paused {
                reason: PauseReason::UserRequested,
            },
            detail: None,
        }));
        assert!(matches!(s.mission_state, Some(MissionState::Paused { .. })));
        assert_eq!(s.mission_detail, None);
    }

    #[test]
    fn workstream_status_creates_panel_and_sets_status() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::WorkstreamStatus {
            id: ws,
            status: WorkstreamStatus::Working,
        });
        assert_eq!(s.workstreams[&ws].status, Some(WorkstreamStatus::Working));
        assert_eq!(s.workstream_order, vec![ws]);
        s.apply(BridgeEvent::WorkstreamStatus {
            id: ws,
            status: WorkstreamStatus::ReadyToMerge,
        });
        assert_eq!(
            s.workstreams[&ws].status,
            Some(WorkstreamStatus::ReadyToMerge)
        );
        assert_eq!(s.workstream_order, vec![ws], "no duplicate order entry");
    }

    #[test]
    fn workstream_first_seen_order_is_stable_under_scrambled_events() {
        let mut s = AppState::default();
        let a = WorkstreamId::new();
        let b = WorkstreamId::new();
        let c = WorkstreamId::new();
        // Scrambled: output for A arrives before any status event; B next via
        // tool call; C last; then more events for A and B must not reorder.
        s.apply(BridgeEvent::AgentOutput {
            workstream: a,
            station: Station::Helm,
            text: "engaging".into(),
        });
        s.apply(BridgeEvent::ToolCall {
            workstream: b,
            station: Station::Science,
            tool_name: "Read".into(),
            summary: "src/lib.rs".into(),
        });
        s.apply(BridgeEvent::WorkstreamStatus {
            id: c,
            status: WorkstreamStatus::Pending,
        });
        s.apply(BridgeEvent::WorkstreamStatus {
            id: b,
            status: WorkstreamStatus::Working,
        });
        s.apply(BridgeEvent::TurnCompleted(turn(a)));
        assert_eq!(s.workstream_order, vec![a, b, c]);
        // Panel data landed in the right panels despite scrambling.
        assert_eq!(s.workstreams[&a].output.len(), 1);
        assert_eq!(s.workstreams[&a].turns.len(), 1);
        assert_eq!(s.workstreams[&b].tool_calls.len(), 1);
        assert_eq!(s.workstreams[&a].status, None, "no status seen for A yet");
    }

    #[test]
    fn agent_output_bounded_per_workstream() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        for i in 0..(MAX_FEED + 3) {
            s.apply(BridgeEvent::AgentOutput {
                workstream: ws,
                station: Station::Helm,
                text: format!("t{i}"),
            });
        }
        let panel = &s.workstreams[&ws];
        assert_eq!(panel.output.len(), MAX_FEED);
        assert_eq!(panel.output[0].1, "t3");
    }

    #[test]
    fn tool_call_appends_summary_line() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::ToolCall {
            workstream: ws,
            station: Station::Helm,
            tool_name: "Bash".into(),
            summary: "cargo test".into(),
        });
        let line = &s.workstreams[&ws].tool_calls[0];
        assert!(line.contains("Bash"), "line: {line}");
        assert!(line.contains("cargo test"), "line: {line}");
    }

    #[test]
    fn turn_completed_appends_to_owning_panel() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        let other = WorkstreamId::new();
        s.apply(BridgeEvent::TurnCompleted(turn(ws)));
        s.apply(BridgeEvent::TurnCompleted(turn(other)));
        s.apply(BridgeEvent::TurnCompleted(turn(ws)));
        assert_eq!(s.workstreams[&ws].turns.len(), 2);
        assert_eq!(s.workstreams[&other].turns.len(), 1);
    }

    #[test]
    fn hook_decisions_feed_bounded_drop_oldest() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        for i in 0..(MAX_FEED + 2) {
            s.apply(BridgeEvent::HookDecision(hook_decision(
                ws,
                &format!("r{i}"),
            )));
        }
        assert_eq!(s.tactical_feed.len(), MAX_FEED);
        assert_eq!(s.tactical_feed[0].rule, "r2");
    }

    #[test]
    fn escalation_requested_adds_once() {
        let mut s = AppState::default();
        let id = EscalationId::new();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::EscalationRequested(ticket(id, ws)));
        s.apply(BridgeEvent::EscalationRequested(ticket(id, ws)));
        assert_eq!(s.escalations.len(), 1, "duplicate ticket ids collapse");
    }

    #[test]
    fn escalation_resolved_removes_ticket() {
        let mut s = AppState::default();
        let keep = EscalationId::new();
        let gone = EscalationId::new();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::EscalationRequested(ticket(keep, ws)));
        s.apply(BridgeEvent::EscalationRequested(ticket(gone, ws)));
        s.apply(BridgeEvent::EscalationResolved {
            id: gone,
            decision: UserDecision::Approve,
        });
        assert_eq!(s.escalations.len(), 1);
        assert_eq!(s.escalations[0].id, keep);
        // Unknown id is a no-op, never a panic.
        s.apply(BridgeEvent::EscalationResolved {
            id: EscalationId::new(),
            decision: UserDecision::Deny {
                reason: "no".into(),
            },
        });
        assert_eq!(s.escalations.len(), 1);
    }

    #[test]
    fn rate_limit_hit_stored_and_cleared_clears() {
        let mut s = AppState::default();
        let at = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        s.apply(BridgeEvent::RateLimit(RateLimitState::Hit {
            retry_at: Some(at),
        }));
        assert_eq!(
            s.rate_limit,
            Some(RateLimitState::Hit { retry_at: Some(at) })
        );
        s.apply(BridgeEvent::RateLimit(RateLimitState::Cleared));
        assert_eq!(s.rate_limit, None, "Cleared clears the banner entirely");
    }

    #[test]
    fn budget_update_replaces_snapshot() {
        let mut s = AppState::default();
        s.apply(BridgeEvent::BudgetUpdate(snapshot(5, 100)));
        s.apply(BridgeEvent::BudgetUpdate(snapshot(6, 100)));
        assert_eq!(s.budget.as_ref().unwrap().total_turns, 6);
    }

    #[test]
    fn merge_queue_update_replaces_whole_snapshot() {
        let mut s = AppState::default();
        let ws1 = WorkstreamId::new();
        let ws2 = WorkstreamId::new();
        let entry = |ws, pos| MergeQueueEntry {
            workstream: ws,
            branch: "b".into(),
            position: pos,
            state: MergeQueueState::AwaitingRebase,
        };
        s.apply(BridgeEvent::MergeQueueUpdate(vec![
            entry(ws1, 0),
            entry(ws2, 1),
        ]));
        assert_eq!(s.merge_queue.len(), 2);
        s.apply(BridgeEvent::MergeQueueUpdate(vec![entry(ws2, 0)]));
        assert_eq!(s.merge_queue.len(), 1);
        assert_eq!(s.merge_queue[0].workstream, ws2);
    }

    #[test]
    fn merge_confirmation_requested_dedupes_by_workstream() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::MergeConfirmationRequested(proposal(ws, "v1")));
        s.apply(BridgeEvent::MergeConfirmationRequested(proposal(ws, "v2")));
        assert_eq!(s.pending_merges.len(), 1);
        assert_eq!(s.pending_merges[0].summary, "v2", "latest proposal wins");
    }

    #[test]
    fn merged_or_failed_status_drops_pending_proposal() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        let other = WorkstreamId::new();
        s.apply(BridgeEvent::MergeConfirmationRequested(proposal(ws, "m")));
        s.apply(BridgeEvent::MergeConfirmationRequested(proposal(
            other, "o",
        )));
        s.apply(BridgeEvent::WorkstreamStatus {
            id: ws,
            status: WorkstreamStatus::Merged,
        });
        assert_eq!(s.pending_merges.len(), 1);
        assert_eq!(s.pending_merges[0].workstream, other);
        s.apply(BridgeEvent::WorkstreamStatus {
            id: other,
            status: WorkstreamStatus::Failed { reason: "x".into() },
        });
        assert!(s.pending_merges.is_empty());
    }

    #[test]
    fn battle_reports_append() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        let report = |round| BattleReport {
            workstream: ws,
            round,
            verdict: Verdict::Clean,
            findings: vec![],
        };
        s.apply(BridgeEvent::BattleReportFiled(report(1)));
        s.apply(BridgeEvent::BattleReportFiled(report(2)));
        assert_eq!(s.battle_reports.len(), 2);
        assert_eq!(s.battle_reports[1].round, 2);
    }

    #[test]
    fn compat_warning_stored() {
        let mut s = AppState::default();
        s.apply(BridgeEvent::CompatWarning {
            detected: "2.3.0".into(),
            tested_min: "2.1.190".into(),
            tested_max: "2.2.99".into(),
        });
        assert_eq!(
            s.compat_warning,
            Some(("2.3.0".into(), "2.1.190".into(), "2.2.99".into()))
        );
    }

    #[test]
    fn remove_merge_proposal_is_local_caller_side_removal() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::MergeConfirmationRequested(proposal(ws, "m")));
        s.remove_merge_proposal(ws);
        assert!(s.pending_merges.is_empty());
    }

    #[test]
    fn remove_escalation_is_local_caller_side_removal() {
        let mut s = AppState::default();
        let id = EscalationId::new();
        s.apply(BridgeEvent::EscalationRequested(ticket(
            id,
            WorkstreamId::new(),
        )));
        s.ui.deny_reasons.insert(id, "because".into());
        s.remove_escalation(id);
        assert!(s.escalations.is_empty());
        assert!(s.ui.deny_reasons.is_empty());
    }

    #[test]
    fn rate_limit_countdown_text_variants() {
        let mut s = AppState::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        assert_eq!(s.rate_limit_countdown_text(now), None);

        s.apply(BridgeEvent::RateLimit(RateLimitState::Hit {
            retry_at: Some(now + Duration::seconds(200)),
        }));
        assert_eq!(
            s.rate_limit_countdown_text(now).as_deref(),
            Some("Rate limited - retrying in 3m 20s")
        );

        s.apply(BridgeEvent::RateLimit(RateLimitState::Hit {
            retry_at: Some(now - Duration::seconds(5)),
        }));
        assert_eq!(
            s.rate_limit_countdown_text(now).as_deref(),
            Some("Rate limited - retrying now")
        );

        s.apply(BridgeEvent::RateLimit(RateLimitState::Hit {
            retry_at: None,
        }));
        assert_eq!(
            s.rate_limit_countdown_text(now).as_deref(),
            Some("Rate limited - waiting for reset")
        );

        s.apply(BridgeEvent::RateLimit(RateLimitState::Cleared));
        assert_eq!(s.rate_limit_countdown_text(now), None);
    }

    #[test]
    fn escalation_countdown_floors_at_zero() {
        let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        let id = EscalationId::new();
        let mut t = ticket(id, WorkstreamId::new());
        t.expires_at = now + Duration::seconds(90);
        assert_eq!(escalation_remaining_secs(&t, now), 90);
        t.expires_at = now - Duration::seconds(10);
        assert_eq!(escalation_remaining_secs(&t, now), 0);
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration_secs(45), "45s");
        assert_eq!(format_duration_secs(200), "3m 20s");
        assert_eq!(format_duration_secs(3720), "1h 02m");
        assert_eq!(format_duration_secs(0), "0s");
    }

    #[test]
    fn workstream_provisioned_sets_path() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::WorkstreamProvisioned {
            id: ws,
            worktree_path: std::path::PathBuf::from("/tmp/wt/ws-a"),
        });
        assert_eq!(
            s.workstreams[&ws].worktree_path.as_deref(),
            Some(std::path::Path::new("/tmp/wt/ws-a"))
        );
    }

    #[test]
    fn status_labels_are_stable() {
        assert_eq!(status_label(&WorkstreamStatus::Pending), "Pending");
        assert_eq!(
            status_label(&WorkstreamStatus::UnderTest { round: 2 }),
            "Under test - round 2"
        );
        assert_eq!(
            status_label(&WorkstreamStatus::Breached { round: 3 }),
            "Breached - round 3"
        );
        assert_eq!(status_label(&WorkstreamStatus::Flagged), "Flagged");
        assert_eq!(
            status_label(&WorkstreamStatus::Failed { reason: "x".into() }),
            "Failed"
        );
    }

    #[test]
    fn mission_state_labels() {
        assert_eq!(mission_state_label(&MissionState::Planning), "Planning");
        assert_eq!(
            mission_state_label(&MissionState::Paused {
                reason: PauseReason::BudgetExhausted {
                    which: "max_total_turns".into()
                }
            }),
            "Paused - budget exhausted (max_total_turns)"
        );
        assert_eq!(
            mission_state_label(&MissionState::Failed {
                reason: "engine down".into()
            }),
            "Failed - engine down"
        );
    }

    #[test]
    fn captain_conversation_events_feed_transcript_and_proposal() {
        let mut s = AppState::default();
        let mission = MissionId::new();
        s.apply(BridgeEvent::UserSaid {
            mission,
            text: "build X".into(),
        });
        s.apply(BridgeEvent::CaptainSays {
            mission,
            text: "Aye.".into(),
        });
        assert_eq!(
            s.captain_feed,
            vec![
                (CaptainSpeaker::You, "build X".to_string()),
                (CaptainSpeaker::Captain, "Aye.".to_string()),
            ]
        );
        let plan = PlanDraft {
            workstreams: vec![PlanDraftWorkstream {
                slug: "a".into(),
                title: "A".into(),
                description: "d".into(),
                depends_on: vec![],
            }],
        };
        s.apply(BridgeEvent::PlanProposed {
            mission,
            revision: 1,
            plan: plan.clone(),
            diff: None,
        });
        assert_eq!(s.latest_proposal.as_ref().unwrap().revision, 1);
        s.apply(BridgeEvent::ProposalRejected {
            mission,
            revision: 1,
            reason: "stale".into(),
        });
        assert!(matches!(
            s.captain_feed.last().unwrap().0,
            CaptainSpeaker::Captain
        ));
        // Launch clears the card.
        s.apply(BridgeEvent::MissionStatus(MissionStatusUpdate {
            mission,
            state: MissionState::Executing,
            detail: None,
        }));
        assert!(s.latest_proposal.is_none());
    }

    #[test]
    fn cancelled_is_terminal_for_pending_merges() {
        let mut s = AppState::default();
        let ws = WorkstreamId::new();
        s.apply(BridgeEvent::WorkstreamStatus {
            id: ws,
            status: WorkstreamStatus::Cancelled,
        });
        assert_eq!(s.workstreams[&ws].status, Some(WorkstreamStatus::Cancelled));
        assert_eq!(status_label(&WorkstreamStatus::Cancelled), "Cancelled");
    }
}
