//! Mission controller integration tests: full mission flows against the
//! mock ports, with paused tokio time (no sleeps, no subprocesses).

use super::*;
use crate::testutil::{GitCall, MockDeps, TurnResponse};
use bridge_core::{BattleReport, Verdict};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Barrier;

const WAIT: Duration = Duration::from_secs(300);

struct Rig {
    deps: Arc<MockDeps>,
    cmd: mpsc::Sender<BridgeCommand>,
    rx: broadcast::Receiver<BridgeEvent>,
    handle: tokio::task::JoinHandle<Result<(), MissionError>>,
    collected: Vec<BridgeEvent>,
}

fn spawn_rig(deps: Arc<MockDeps>, config: BridgeConfig, plan: Option<MissionPlan>) -> Rig {
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (ev_tx, ev_rx) = broadcast::channel(4096);
    let mut controller = MissionController::new(
        deps.clone(),
        config,
        PathBuf::from("/opt/bridge/bridge-hook-helper"),
        ev_tx,
        cmd_rx,
    );
    if let Some(plan) = plan {
        controller = controller.with_resumed_plan(plan);
    }
    let handle = tokio::spawn(controller.run());
    Rig {
        deps,
        cmd: cmd_tx,
        rx: ev_rx,
        handle,
        collected: Vec::new(),
    }
}

impl Rig {
    async fn send(&self, cmd: BridgeCommand) {
        self.cmd.send(cmd).await.expect("controller alive");
    }

    async fn wait_for(&mut self, what: &str, pred: impl Fn(&BridgeEvent) -> bool) -> BridgeEvent {
        loop {
            let ev = tokio::time::timeout(WAIT, self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .expect("event bus closed");
            self.collected.push(ev.clone());
            if pred(&ev) {
                return ev;
            }
        }
    }

    /// Poll a mock-side condition; paused time makes this deterministic.
    async fn wait_until(&self, what: &str, cond: impl Fn() -> bool) {
        for _ in 0..2000 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting until {what}");
    }

    async fn start(&mut self, objective: &str) {
        self.send(BridgeCommand::SayToCaptain {
            text: objective.into(),
        })
        .await;
        let proposed = self
            .wait_for("plan proposed", |e| {
                matches!(e, BridgeEvent::PlanProposed { .. })
            })
            .await;
        let BridgeEvent::PlanProposed { revision, .. } = proposed else {
            unreachable!()
        };
        self.send(BridgeCommand::ApproveProposal { revision }).await;
        self.wait_for("mission Executing", |e| {
            matches!(
                e,
                BridgeEvent::MissionStatus(MissionStatusUpdate {
                    state: MissionState::Executing,
                    ..
                })
            )
        })
        .await;
    }

    fn plan(&self) -> MissionPlan {
        self.deps
            .recorded_missions
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|p| !p.workstreams.is_empty())
            .cloned()
            .expect("a real plan was recorded")
    }

    fn ws_id(&self, slug: &str) -> WorkstreamId {
        self.plan()
            .workstreams
            .iter()
            .find(|w| w.slug == slug)
            .unwrap_or_else(|| panic!("no workstream {slug}"))
            .id
    }

    async fn finish(self) -> Result<(), MissionError> {
        tokio::time::timeout(WAIT, self.handle)
            .await
            .expect("controller did not stop")
            .expect("controller task panicked")
    }

    async fn shutdown(self) -> Result<(), MissionError> {
        self.send(BridgeCommand::Shutdown).await;
        self.finish().await
    }

    fn proposals(&self) -> Vec<WorkstreamId> {
        self.collected
            .iter()
            .filter_map(|e| match e {
                BridgeEvent::MergeConfirmationRequested(p) => Some(p.workstream),
                _ => None,
            })
            .collect()
    }
}

fn breached_report_json(title: &str) -> serde_json::Value {
    serde_json::json!({
        "verdict": "breached",
        "findings": [{
            "severity": "high",
            "title": title,
            "description": "it broke",
            "reproduction_command": format!("cargo test {title}"),
            "failing_test_path": "tests/adversarial/breach.rs",
            "weakness_class": "logic-error"
        }]
    })
}

// ---------------------------------------------------------------------------
// 1. the full happy path with two independent workstreams

#[tokio::test(start_paused = true)]
async fn full_mission_two_parallel_workstreams() {
    let deps = MockDeps::new();
    deps.set_plan(&[("part-a", &[]), ("part-b", &[])]);
    // Both Helm turns must be in flight at the same time to pass this.
    *deps.helm_barrier.lock().unwrap() = Some(Arc::new(Barrier::new(2)));
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);

    rig.start("ship both features").await;

    // Captain turn carried the PlanDraft schema.
    let captain_turns: Vec<_> = deps
        .turn_log()
        .into_iter()
        .filter(|t| t.station == Station::Captain)
        .collect();
    assert_eq!(captain_turns.len(), 1);
    assert_eq!(
        captain_turns[0].inv.json_schema,
        Some(CaptainReply::json_schema())
    );

    // First (and so far only) merge proposal: the queue is serialized.
    rig.wait_for("first merge proposal", |e| {
        matches!(e, BridgeEvent::MergeConfirmationRequested(_))
    })
    .await;
    assert_eq!(rig.proposals().len(), 1, "only the queue head may propose");

    // Both worktrees were provisioned and both Helm turns ran (the barrier
    // proves concurrency: sequential dispatch would deadlock).
    let git = deps.git_log();
    for slug in ["part-a", "part-b"] {
        assert!(git.iter().any(|c| matches!(
            c,
            GitCall::CreateWorktree { ws, .. } if ws == slug
        )));
    }
    let kobayashi: Vec<_> = deps
        .turn_log()
        .into_iter()
        .filter(|t| t.station == Station::KobayashiMaru)
        .collect();
    assert_eq!(kobayashi.len(), 2);
    for t in &kobayashi {
        assert!(t.kobayashi, "kobayashi flag must be set on the TurnCtx");
        assert_eq!(t.inv.resume, None, "kobayashi rounds never resume");
    }

    // MergeQueueUpdate events were observed.
    assert!(
        rig.collected
            .iter()
            .any(|e| matches!(e, BridgeEvent::MergeQueueUpdate(entries) if !entries.is_empty()))
    );

    // Approve the head; the second proposal only arrives after the first
    // merge completes.
    let first = rig.proposals()[0];
    rig.send(BridgeCommand::ConfirmMerge {
        workstream: first,
        approved: true,
    })
    .await;
    rig.wait_for("first workstream merged", |e| {
        matches!(e, BridgeEvent::WorkstreamStatus { id, status: WorkstreamStatus::Merged } if *id == first)
    })
    .await;
    rig.wait_for(
        "second merge proposal",
        |e| matches!(e, BridgeEvent::MergeConfirmationRequested(p) if p.workstream != first),
    )
    .await;
    let second = *rig.proposals().last().unwrap();
    assert_ne!(first, second);

    // The remaining workstream was rebased onto the new main after the
    // first merge (its quiet point in the queue).
    let git = deps.git_log();
    let plan = rig.plan();
    let second_slug = &plan
        .workstreams
        .iter()
        .find(|w| w.id == second)
        .unwrap()
        .slug;
    let second_branch = format!("bridge/{}/{}", plan.slug, second_slug);
    let merge_idx = git
        .iter()
        .position(|c| matches!(c, GitCall::Merge { .. }))
        .expect("first merge recorded");
    let rebase_idx = git
        .iter()
        .position(|c| matches!(c, GitCall::Rebase { branch, target } if *branch == second_branch && target == "main"))
        .expect("second workstream rebased onto main");
    assert!(
        merge_idx < rebase_idx,
        "post-merge rebase must follow the merge"
    );

    rig.send(BridgeCommand::ConfirmMerge {
        workstream: second,
        approved: true,
    })
    .await;
    rig.wait_for("mission complete", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Complete,
                ..
            })
        )
    })
    .await;

    // Complete only after the Comms report turn.
    let turns = deps.turn_log();
    assert_eq!(turns.last().unwrap().station, Station::Comms);

    // Both merges happened, in confirmation order.
    let merges: Vec<_> = deps
        .git_log()
        .into_iter()
        .filter(|c| matches!(c, GitCall::Merge { .. }))
        .collect();
    assert_eq!(merges.len(), 2);

    // Statuses were recorded through the ComputerPort at every change.
    let expected = [
        WorkstreamStatus::Pending,
        WorkstreamStatus::Working,
        WorkstreamStatus::UnderTest { round: 1 },
        WorkstreamStatus::ReadyToMerge,
        WorkstreamStatus::InMergeQueue,
        WorkstreamStatus::Rebasing,
        WorkstreamStatus::InMergeQueue,
        WorkstreamStatus::Merged,
    ];
    assert_eq!(deps.statuses_for(first), expected.to_vec());
    assert_eq!(deps.statuses_for(second), expected.to_vec());

    // TurnRecords recorded: captain + two helm + comms (kobayashi rounds
    // are recorded as battle reports instead).
    let recorded: Vec<Station> = deps
        .recorded_turns
        .lock()
        .unwrap()
        .iter()
        .map(|(_, t)| t.station)
        .collect();
    assert_eq!(
        recorded.iter().filter(|s| **s == Station::Helm).count(),
        2,
        "both helm turns recorded"
    );
    assert!(recorded.contains(&Station::Captain));
    assert!(recorded.contains(&Station::Comms));
    assert_eq!(deps.recorded_reports.lock().unwrap().len(), 2);
    assert_eq!(deps.recorded_completes.lock().unwrap().len(), 1);

    assert!(matches!(rig.finish().await, Ok(())));

    // Both merged workstreams' implementation worktrees are decommissioned
    // on mission end (disarmed from Tactical + force-removed). Throwaway
    // Kobayashi worktrees are removed mid-mission; here we assert the two
    // implementation worktrees are gone and both workstreams disarmed.
    let disarmed = deps.disarmed.lock().unwrap().clone();
    assert!(
        disarmed.contains(&first) && disarmed.contains(&second),
        "both disarmed: {disarmed:?}"
    );
    let impl_removed = deps
        .git_log()
        .into_iter()
        .filter(|c| matches!(c, GitCall::RemoveWorktree { path, .. } if !path.to_string_lossy().contains("throwaway")))
        .count();
    assert_eq!(
        impl_removed, 2,
        "both implementation worktrees removed on mission end"
    );
}

// ---------------------------------------------------------------------------
// 2. breach -> fix in the ORIGINAL worktree -> flagged after max rounds ->
//    user override enters the queue

#[tokio::test(start_paused = true)]
async fn breached_rounds_fix_in_original_worktree_then_flag_and_override() {
    let deps = MockDeps::new();
    for round in 1..=3 {
        deps.push_turn(
            Station::KobayashiMaru,
            TurnResponse::structured(breached_report_json(&format!("hole-{round}"))),
        );
    }
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("breach mission").await;
    let ws = rig.ws_id("solo");

    rig.wait_for("workstream flagged", |e| {
        matches!(
            e,
            BridgeEvent::WorkstreamStatus {
                status: WorkstreamStatus::Flagged,
                ..
            }
        )
    })
    .await;

    // No merge proposal for a flagged workstream.
    assert!(
        rig.proposals().is_empty(),
        "flagged workstream must not propose a merge"
    );

    // One work order + two fix orders, all in the ORIGINAL worktree, with
    // the session resumed from the first turn (continuity).
    let helm: Vec<_> = deps
        .turn_log()
        .into_iter()
        .filter(|t| t.station == Station::Helm)
        .collect();
    assert_eq!(helm.len(), 3, "work + 2 fix orders (3rd breach flags)");
    let worktree = deps.root.path().join("breach-mission").join("solo");
    for t in &helm {
        assert_eq!(
            t.inv.cwd, worktree,
            "fix orders run in the original worktree"
        );
    }
    assert_eq!(helm[0].inv.resume, None);
    for fix in &helm[1..] {
        assert_eq!(
            fix.inv.resume,
            Some(SessionId("sess-solo".into())),
            "fix orders resume the recorded helm session"
        );
        assert!(
            fix.inv.prompt.contains("hole-"),
            "fix prompt lists the findings"
        );
    }

    // Three fresh kobayashi rounds (fresh session each).
    let kobayashi: Vec<_> = deps
        .turn_log()
        .into_iter()
        .filter(|t| t.station == Station::KobayashiMaru)
        .collect();
    assert_eq!(kobayashi.len(), 3);
    assert!(kobayashi.iter().all(|t| t.inv.resume.is_none()));

    // Status history up to the flag.
    assert_eq!(
        deps.statuses_for(ws),
        vec![
            WorkstreamStatus::Pending,
            WorkstreamStatus::Working,
            WorkstreamStatus::UnderTest { round: 1 },
            WorkstreamStatus::Breached { round: 1 },
            WorkstreamStatus::UnderTest { round: 2 },
            WorkstreamStatus::Breached { round: 2 },
            WorkstreamStatus::UnderTest { round: 3 },
            WorkstreamStatus::Flagged,
        ]
    );

    // Explicit user override lets it enter the merge queue.
    rig.send(BridgeCommand::OverrideFlagged { workstream: ws })
        .await;
    rig.wait_for("flagged workstream enqueued", |e| {
        matches!(e, BridgeEvent::MergeQueueUpdate(entries) if entries.iter().any(|q| q.workstream == ws))
    })
    .await;
    rig.wait_for(
        "merge proposal after override",
        |e| matches!(e, BridgeEvent::MergeConfirmationRequested(p) if p.workstream == ws),
    )
    .await;

    assert!(matches!(rig.shutdown().await, Ok(())));
}

// ---------------------------------------------------------------------------
// 3. rebase conflict in the merge queue -> conflict fix -> fresh kobayashi
//    round BEFORE the confirmation

#[tokio::test(start_paused = true)]
async fn rebase_conflict_gets_fix_order_and_fresh_round_before_confirmation() {
    let deps = MockDeps::new();
    deps.push_rebase(RebaseOutcome::Conflicts {
        files: vec![PathBuf::from("src/x.rs")],
    });
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("conflicted mission").await;
    let ws = rig.ws_id("solo");

    rig.wait_for("merge proposal", |e| {
        matches!(e, BridgeEvent::MergeConfirmationRequested(_))
    })
    .await;

    // A conflict-fix Helm order ran in the original worktree naming the file.
    let helm: Vec<_> = deps
        .turn_log()
        .into_iter()
        .filter(|t| t.station == Station::Helm)
        .collect();
    assert_eq!(helm.len(), 2, "work order + conflict fix order");
    let fix = &helm[1];
    assert!(fix.inv.prompt.contains("src/x.rs"));
    assert_eq!(
        fix.inv.cwd,
        deps.root.path().join("conflicted-mission").join("solo")
    );

    // A FRESH kobayashi round ran after the fix, before the confirmation.
    let kobayashi_count = deps
        .turn_log()
        .iter()
        .filter(|t| t.station == Station::KobayashiMaru)
        .count();
    assert_eq!(kobayashi_count, 2, "initial round + post-conflict round");

    // Conflict status was surfaced.
    let statuses = deps.statuses_for(ws);
    assert!(statuses.contains(&WorkstreamStatus::ConflictFix));
    assert!(statuses.contains(&WorkstreamStatus::UnderTest { round: 2 }));

    rig.send(BridgeCommand::ConfirmMerge {
        workstream: ws,
        approved: true,
    })
    .await;
    rig.wait_for("mission complete", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Complete,
                ..
            })
        )
    })
    .await;
    assert!(matches!(rig.finish().await, Ok(())));
}

// ---------------------------------------------------------------------------
// 4. budget exhaustion pauses BEFORE dispatch; ExtendBudget resumes

#[tokio::test(start_paused = true)]
async fn budget_exhaustion_pauses_dispatch_and_extend_resumes() {
    let deps = MockDeps::new();
    let mut config = BridgeConfig::default();
    // Captain conference turns are governed by captain.max_conference_turns,
    // not this budget, so the gate must bite before the first Helm turn.
    config.budgets.max_total_turns = 0;
    let mut rig = spawn_rig(deps.clone(), config, None);
    rig.start("tiny budget").await;

    rig.wait_for("budget pause", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Paused { reason: PauseReason::BudgetExhausted { which } },
                ..
            }) if which == "max_total_turns"
        )
    })
    .await;

    // No TurnPort call beyond the captain, and no provisioning either.
    assert_eq!(deps.turn_log().len(), 1);
    assert_eq!(deps.turn_log()[0].station, Station::Captain);
    assert!(
        !deps
            .git_log()
            .iter()
            .any(|c| matches!(c, GitCall::CreateWorktree { .. }))
    );

    rig.send(BridgeCommand::ExtendBudget(BudgetExtension {
        extra_total_turns: 50,
        extra_turns_per_workstream: 0,
        extra_wall_clock_secs: 0,
    }))
    .await;
    rig.wait_for("resumed", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;
    rig.wait_until("helm turn dispatched after extension", || {
        deps.turn_log().iter().any(|t| t.station == Station::Helm)
    })
    .await;

    assert!(matches!(rig.shutdown().await, Ok(())));
}

// ---------------------------------------------------------------------------
// 5. pre-dispatch screening

#[tokio::test(start_paused = true)]
async fn screen_rejected_fails_workstream_without_any_helm_turn() {
    let deps = MockDeps::new();
    deps.set_screen_fn(|order| {
        if order.station == Station::Helm {
            bridge_tactical::ScreenResult::Rejected {
                reason: "tool overreach".into(),
            }
        } else {
            bridge_tactical::ScreenResult::Cleared
        }
    });
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("rejected mission").await;

    rig.wait_for("workstream failed", |e| {
        matches!(
            e,
            BridgeEvent::WorkstreamStatus { status: WorkstreamStatus::Failed { reason }, .. }
                if reason.contains("tool overreach")
        )
    })
    .await;
    rig.wait_for("mission failed", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate { state: MissionState::Failed { reason }, .. })
                if reason.contains("workstreams failed")
        )
    })
    .await;

    let stations: Vec<Station> = deps.turn_log().iter().map(|t| t.station).collect();
    assert!(
        !stations.contains(&Station::Helm),
        "rejected order must never reach the TurnPort, got {stations:?}"
    );
    assert!(!stations.contains(&Station::KobayashiMaru));
    assert!(matches!(rig.finish().await, Ok(())));
}

#[tokio::test(start_paused = true)]
async fn screen_needs_review_waits_for_escalation_approval() {
    let deps = MockDeps::new();
    let flagged_once = AtomicBool::new(false);
    deps.set_screen_fn(move |order| {
        if order.station == Station::Helm && !flagged_once.swap(true, Ordering::SeqCst) {
            bridge_tactical::ScreenResult::NeedsReview {
                flags: vec!["embedded instruction smell".into()],
            }
        } else {
            bridge_tactical::ScreenResult::Cleared
        }
    });
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("review mission").await;

    let ev = rig
        .wait_for("escalation requested", |e| {
            matches!(e, BridgeEvent::EscalationRequested(_))
        })
        .await;
    let BridgeEvent::EscalationRequested(ticket) = ev else {
        unreachable!()
    };
    assert!(ticket.question.contains("embedded instruction smell"));

    // Dispatch is held: no helm turn yet.
    assert!(!deps.turn_log().iter().any(|t| t.station == Station::Helm));

    rig.send(BridgeCommand::ResolveEscalation {
        id: ticket.id,
        decision: UserDecision::Approve,
    })
    .await;
    rig.wait_for("escalation resolved", |e| {
        matches!(e, BridgeEvent::EscalationResolved { .. })
    })
    .await;
    rig.wait_until("helm turn after approval", || {
        deps.turn_log().iter().any(|t| t.station == Station::Helm)
    })
    .await;

    assert!(matches!(rig.shutdown().await, Ok(())));
}

#[tokio::test(start_paused = true)]
async fn screen_needs_review_denied_fails_the_workstream() {
    let deps = MockDeps::new();
    let flagged_once = AtomicBool::new(false);
    deps.set_screen_fn(move |order| {
        if order.station == Station::Helm && !flagged_once.swap(true, Ordering::SeqCst) {
            bridge_tactical::ScreenResult::NeedsReview {
                flags: vec!["sus".into()],
            }
        } else {
            bridge_tactical::ScreenResult::Cleared
        }
    });
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("denied mission").await;

    let ev = rig
        .wait_for("escalation requested", |e| {
            matches!(e, BridgeEvent::EscalationRequested(_))
        })
        .await;
    let BridgeEvent::EscalationRequested(ticket) = ev else {
        unreachable!()
    };
    rig.send(BridgeCommand::ResolveEscalation {
        id: ticket.id,
        decision: UserDecision::Deny {
            reason: "not comfortable".into(),
        },
    })
    .await;
    rig.wait_for("workstream failed", |e| {
        matches!(
            e,
            BridgeEvent::WorkstreamStatus { status: WorkstreamStatus::Failed { reason }, .. }
                if reason.contains("not comfortable")
        )
    })
    .await;
    assert!(!deps.turn_log().iter().any(|t| t.station == Station::Helm));
    // The failed workstream settles the mission, which ends Failed.
    rig.wait_for("mission failed", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Failed { .. },
                ..
            })
        )
    })
    .await;
    assert!(matches!(rig.finish().await, Ok(())));
}

#[tokio::test(start_paused = true)]
async fn unanswered_screening_escalation_times_out_to_deny() {
    // A screening escalation the user never answers must not wedge the
    // mission: after escalation_timeout_secs it fails closed to Deny.
    let deps = MockDeps::new();
    let flagged_once = AtomicBool::new(false);
    deps.set_screen_fn(move |order| {
        if order.station == Station::Helm && !flagged_once.swap(true, Ordering::SeqCst) {
            bridge_tactical::ScreenResult::NeedsReview {
                flags: vec!["sus".into()],
            }
        } else {
            bridge_tactical::ScreenResult::Cleared
        }
    });
    let mut config = BridgeConfig::default();
    config.tactical.escalation_timeout_secs = 30;
    let mut rig = spawn_rig(deps.clone(), config, None);
    rig.start("timeout mission").await;

    rig.wait_for("escalation requested", |e| {
        matches!(e, BridgeEvent::EscalationRequested(_))
    })
    .await;

    // Advance past the deadline without answering.
    tokio::time::advance(Duration::from_secs(31)).await;

    rig.wait_for("workstream failed on timeout", |e| {
        matches!(
            e,
            BridgeEvent::WorkstreamStatus { status: WorkstreamStatus::Failed { reason }, .. }
                if reason.contains("timed out")
        )
    })
    .await;
    assert!(!deps.turn_log().iter().any(|t| t.station == Station::Helm));
    assert!(matches!(rig.finish().await, Ok(())));
}

// ---------------------------------------------------------------------------
// 5b. a hook (broker) escalation id is forwarded to the control server

#[tokio::test(start_paused = true)]
async fn resolve_escalation_forwards_hook_ticket_to_broker() {
    // A mid-run hook escalation lives on the control server's broker, not in
    // the controller's order-screening map. ResolveEscalation for such an id
    // must be forwarded to the broker (via TacticalPort), otherwise the user
    // can never approve a hook and every one fails closed on timeout.
    let deps = MockDeps::new();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("mission with a hook escalation").await;

    let hook_id = bridge_core::EscalationId::new();
    rig.send(BridgeCommand::ResolveEscalation {
        id: hook_id,
        decision: UserDecision::Approve,
    })
    .await;

    rig.wait_for(
        "hook escalation resolved",
        |e| matches!(e, BridgeEvent::EscalationResolved { id, .. } if *id == hook_id),
    )
    .await;

    rig.wait_until("broker received the resolution", || {
        deps.resolved_hook_escalations
            .lock()
            .unwrap()
            .iter()
            .any(|(id, d)| *id == hook_id && *d == UserDecision::Approve)
    })
    .await;

    let _ = rig.shutdown().await;
}

// ---------------------------------------------------------------------------
// 5c. resync re-emits open actionable signals (broadcast-lag recovery)

#[tokio::test(start_paused = true)]
async fn resync_reemits_open_escalations() {
    let deps = MockDeps::new();
    let flagged_once = AtomicBool::new(false);
    deps.set_screen_fn(move |order| {
        if order.station == Station::Helm && !flagged_once.swap(true, Ordering::SeqCst) {
            bridge_tactical::ScreenResult::NeedsReview {
                flags: vec!["sus".into()],
            }
        } else {
            bridge_tactical::ScreenResult::Cleared
        }
    });
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("resync mission").await;

    let ev = rig
        .wait_for("first escalation", |e| {
            matches!(e, BridgeEvent::EscalationRequested(_))
        })
        .await;
    let BridgeEvent::EscalationRequested(original) = ev else {
        unreachable!()
    };

    // Simulate a consumer that lagged and lost the event: ask for a resync.
    rig.send(BridgeCommand::ResyncActionable).await;
    let ev = rig
        .wait_for(
            "re-emitted escalation",
            |e| matches!(e, BridgeEvent::EscalationRequested(t) if t.id == original.id),
        )
        .await;
    let BridgeEvent::EscalationRequested(reemitted) = ev else {
        unreachable!()
    };
    assert_eq!(
        reemitted.id, original.id,
        "same ticket re-emitted on resync"
    );

    // Resolving it once still works (not duplicated into two live tickets).
    rig.send(BridgeCommand::ResolveEscalation {
        id: original.id,
        decision: UserDecision::Approve,
    })
    .await;
    rig.wait_for(
        "resolved",
        |e| matches!(e, BridgeEvent::EscalationResolved { id, .. } if *id == original.id),
    )
    .await;
    let _ = rig.shutdown().await;
}

// ---------------------------------------------------------------------------
// 6. shutdown mid-mission

#[tokio::test(start_paused = true)]
async fn shutdown_mid_mission_returns_ok_with_no_further_dispatches() {
    let deps = MockDeps::new();
    deps.push_turn(Station::Helm, TurnResponse::Hang);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("doomed mission").await;

    rig.wait_for("helm working", |e| {
        matches!(
            e,
            BridgeEvent::WorkstreamStatus {
                status: WorkstreamStatus::Working,
                ..
            }
        )
    })
    .await;
    rig.wait_until("helm turn in flight", || {
        deps.turn_log().iter().any(|t| t.station == Station::Helm)
    })
    .await;
    let before = deps.turn_log().len();

    let result = rig.shutdown().await;
    assert!(
        matches!(result, Ok(())),
        "shutdown returns Ok, got {result:?}"
    );
    assert_eq!(
        deps.turn_log().len(),
        before,
        "no dispatches after shutdown"
    );
}

// ---------------------------------------------------------------------------
// 7. dependent workstreams wait for their dependency to MERGE

#[tokio::test(start_paused = true)]
async fn dependent_workstream_only_provisioned_after_dependency_merges() {
    let deps = MockDeps::new();
    deps.set_plan(&[("base", &[]), ("addon", &["base"])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("layered mission").await;
    let base = rig.ws_id("base");
    let addon = rig.ws_id("addon");

    rig.wait_for(
        "base merge proposal",
        |e| matches!(e, BridgeEvent::MergeConfirmationRequested(p) if p.workstream == base),
    )
    .await;
    // The dependent has not been touched yet.
    assert!(!deps.git_log().iter().any(|c| matches!(
        c,
        GitCall::CreateWorktree { ws, .. } if ws == "addon"
    )));
    assert_eq!(deps.statuses_for(addon), vec![WorkstreamStatus::Pending]);

    rig.send(BridgeCommand::ConfirmMerge {
        workstream: base,
        approved: true,
    })
    .await;
    rig.wait_for("base merged", |e| {
        matches!(e, BridgeEvent::WorkstreamStatus { id, status: WorkstreamStatus::Merged } if *id == base)
    })
    .await;

    rig.wait_until("addon provisioned after merge", || {
        deps.git_log().iter().any(|c| {
            matches!(
                c,
                GitCall::CreateWorktree { ws, .. } if ws == "addon"
            )
        })
    })
    .await;
    let git = deps.git_log();
    let merge_idx = git
        .iter()
        .position(|c| matches!(c, GitCall::Merge { .. }))
        .unwrap();
    let create_idx = git
        .iter()
        .position(|c| matches!(c, GitCall::CreateWorktree { ws, .. } if ws == "addon"))
        .unwrap();
    assert!(
        merge_idx < create_idx,
        "addon must be provisioned only after base merged"
    );

    rig.wait_for(
        "addon merge proposal",
        |e| matches!(e, BridgeEvent::MergeConfirmationRequested(p) if p.workstream == addon),
    )
    .await;
    rig.send(BridgeCommand::ConfirmMerge {
        workstream: addon,
        approved: true,
    })
    .await;
    rig.wait_for("mission complete", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Complete,
                ..
            })
        )
    })
    .await;
    assert!(matches!(rig.finish().await, Ok(())));
}

// ---------------------------------------------------------------------------
// 8. rate limit pauses dispatch; the scheduled probe resumes it

#[tokio::test(start_paused = true)]
async fn rate_limit_hit_pauses_and_probe_resumes() {
    let deps = MockDeps::new();
    deps.push_turn(
        Station::Helm,
        TurnResponse::rate_limited(RateLimitHit {
            retry_at: None,
            trigger: "rate_limit_event".into(),
        }),
    );
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("throttled mission").await;

    rig.wait_for("rate limit hit", |e| {
        matches!(e, BridgeEvent::RateLimit(RateLimitState::Hit { .. }))
    })
    .await;
    rig.wait_for("paused for rate limit", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Paused {
                    reason: PauseReason::RateLimited { .. }
                },
                ..
            })
        )
    })
    .await;
    // The probe timer (paused clock auto-advances) clears the gate.
    rig.wait_for("rate limit cleared", |e| {
        matches!(e, BridgeEvent::RateLimit(RateLimitState::Cleared))
    })
    .await;
    rig.wait_for("executing again", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;
    // The held-back kobayashi round dispatches after the resume.
    rig.wait_until("kobayashi round after resume", || {
        deps.turn_log()
            .iter()
            .any(|t| t.station == Station::KobayashiMaru)
    })
    .await;

    assert!(matches!(rig.shutdown().await, Ok(())));
}

// ---------------------------------------------------------------------------
// resumed plans skip the captain

#[tokio::test(start_paused = true)]
async fn resumed_plan_skips_captain_and_starts_working() {
    let deps = MockDeps::new();
    let draft: PlanDraft =
        serde_json::from_value(crate::testutil::draft_json(&[("solo", &[])])).unwrap();
    let plan = MissionPlan::from_draft(
        draft,
        MissionId::new(),
        "resumed-mission",
        "carry on",
        "main",
    )
    .unwrap();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), Some(plan));

    rig.wait_for("executing", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;
    rig.wait_until("helm turn", || !deps.turn_log().is_empty())
        .await;
    assert_eq!(
        deps.turn_log()[0].station,
        Station::Helm,
        "no captain turn on resume"
    );

    rig.wait_for("merge proposal", |e| {
        matches!(e, BridgeEvent::MergeConfirmationRequested(_))
    })
    .await;
    assert!(matches!(rig.shutdown().await, Ok(())));
}

// ---------------------------------------------------------------------------
// battle report events

#[tokio::test(start_paused = true)]
async fn battle_reports_are_filed_on_the_bus() {
    let deps = MockDeps::new();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("report mission").await;
    let ev = rig
        .wait_for("battle report filed", |e| {
            matches!(e, BridgeEvent::BattleReportFiled(_))
        })
        .await;
    let BridgeEvent::BattleReportFiled(report) = ev else {
        unreachable!()
    };
    let expected: BattleReport = BattleReport {
        workstream: rig.ws_id("solo"),
        round: 1,
        verdict: Verdict::Clean,
        findings: vec![],
    };
    assert_eq!(report, expected);
    assert!(matches!(rig.shutdown().await, Ok(())));
}

// ---------------------------------------------------------------------------
// worktree provisioning events

#[tokio::test(start_paused = true)]
async fn provisioning_emits_worktree_path() {
    let deps = MockDeps::new();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("provision mission").await;
    rig.wait_for("worktree path emitted", |e| {
        matches!(e, BridgeEvent::WorkstreamProvisioned { worktree_path, .. }
            if worktree_path.to_string_lossy().contains("provision-mission"))
    })
    .await;
    let _ = rig.shutdown().await;
}

// ---------------------------------------------------------------------------
// pre-launch Captain conference

#[tokio::test(start_paused = true)]
async fn conference_converses_then_proposes_then_launches_on_approval() {
    let deps = MockDeps::new();
    deps.set_plan(&[("solo", &[])]);
    // First turn: pure conversation, no plan.
    deps.push_turn(
        Station::Captain,
        TurnResponse::structured(serde_json::json!({
            "message": "What does done look like?",
        })),
    );
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);

    rig.send(BridgeCommand::SayToCaptain {
        text: "ship it".into(),
    })
    .await;
    rig.wait_for(
        "user echo",
        |e| matches!(e, BridgeEvent::UserSaid { text, .. } if text == "ship it"),
    )
    .await;
    rig.wait_for(
        "captain question",
        |e| matches!(e, BridgeEvent::CaptainSays { text, .. } if text.contains("done")),
    )
    .await;
    // No proposal yet; approving revision 1 must be rejected.
    rig.send(BridgeCommand::ApproveProposal { revision: 1 })
        .await;
    rig.wait_for("no-proposal rejection", |e| {
        matches!(e, BridgeEvent::ProposalRejected { .. })
    })
    .await;

    // Second exchange: default response proposes the plan.
    rig.send(BridgeCommand::SayToCaptain {
        text: "tests pass".into(),
    })
    .await;
    let BridgeEvent::PlanProposed { revision, diff, .. } = rig
        .wait_for("proposal", |e| {
            matches!(e, BridgeEvent::PlanProposed { .. })
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(revision, 1);
    assert!(diff.is_none(), "pre-launch proposal has no diff");

    rig.send(BridgeCommand::ApproveProposal { revision }).await;
    rig.wait_for("executing", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;

    // Two conference turns ran, both resumable-schema'd; second resumed.
    let captain: Vec<_> = deps
        .turn_log()
        .into_iter()
        .filter(|t| t.station == Station::Captain)
        .collect();
    assert_eq!(captain.len(), 2);
    assert_eq!(captain[0].inv.resume, None);
    assert_eq!(captain[1].inv.resume, Some(SessionId::from("mock-session")));
    // Conference turns recorded to the computer against the mission.
    assert!(deps.recorded_turns.lock().unwrap().len() >= 2);
    // Session persisted with repo_root cwd.
    {
        let sessions = deps.recorded_sessions.lock().unwrap();
        assert!(!sessions.is_empty());
        assert_eq!(sessions[0].1, Station::Captain);
    }

    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn in_flight_turn_proposal_discarded_after_approval() {
    let deps = MockDeps::new();
    deps.set_plan(&[("keep", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() })
        .await;
    rig.wait_for("rev 1", |e| {
        matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })
    })
    .await;

    // Hold the next captain turn open at a barrier while we approve rev 1.
    let barrier = Arc::new(Barrier::new(2));
    *deps.captain_barrier.lock().unwrap() = Some(barrier.clone());
    deps.push_turn(
        Station::Captain,
        TurnResponse::structured(serde_json::json!({
            "message": "Late proposal incoming.",
            "proposed_plan": { "workstreams": [
                { "slug": "late", "title": "Late", "description": "Late", "depends_on": [] }
            ]}
        })),
    );
    rig.send(BridgeCommand::SayToCaptain {
        text: "one more tweak".into(),
    })
    .await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 })
        .await;
    rig.wait_for("executing", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;

    // Release the held turn; its message must surface, its proposal must not.
    barrier.wait().await;
    rig.wait_for("late message emitted", |e| {
        matches!(e, BridgeEvent::CaptainSays { text, .. } if text.contains("Late proposal incoming"))
    })
    .await;
    // `captain_done` for the late turn keeps running past the CaptainSays
    // emission (its proposal handling awaits a blocking call), so the event
    // above lands before that processing finishes. Force the event loop to
    // fully drain it: commands are handled strictly sequentially, so a
    // sentinel with a revision that can never match is only processed after
    // the late turn's captain_done call has completely finished.
    rig.send(BridgeCommand::ApproveProposal { revision: 999 })
        .await;
    rig.wait_for("sentinel rejected", |e| {
        matches!(e, BridgeEvent::ProposalRejected { revision: 999, .. })
    })
    .await;
    assert!(
        !rig.collected
            .iter()
            .any(|e| matches!(e, BridgeEvent::PlanProposed { revision: 2, .. })),
        "in-flight proposal must be discarded after approval"
    );
    // Launched plan is rev 1's.
    assert_eq!(rig.plan().workstreams[0].slug, "keep");
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stale_approval_rejected_and_does_not_launch() {
    let deps = MockDeps::new();
    deps.set_plan(&[("v1", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() })
        .await;
    rig.wait_for("rev 1", |e| {
        matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })
    })
    .await;
    // Second proposal supersedes.
    deps.set_plan(&[("v2", &[])]);
    rig.send(BridgeCommand::SayToCaptain {
        text: "actually, rename it".into(),
    })
    .await;
    rig.wait_for("rev 2", |e| {
        matches!(e, BridgeEvent::PlanProposed { revision: 2, .. })
    })
    .await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 })
        .await;
    rig.wait_for("stale rejected", |e| {
        matches!(e, BridgeEvent::ProposalRejected { revision: 1, .. })
    })
    .await;
    assert!(
        deps.recorded_missions.lock().unwrap()[0]
            .workstreams
            .is_empty(),
        "nothing launched"
    );
    rig.send(BridgeCommand::ApproveProposal { revision: 2 })
        .await;
    rig.wait_for("executing", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;
    assert_eq!(rig.plan().workstreams[0].slug, "v2");
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn approval_during_in_flight_turn_locks_and_discards_late_proposal() {
    let deps = MockDeps::new();
    deps.set_plan(&[("keep", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() })
        .await;
    rig.wait_for("rev 1", |e| {
        matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })
    })
    .await;
    // A turn that never completes keeps the conference in-flight
    // deterministically while we approve rev 1 underneath it.
    deps.push_turn(Station::Captain, TurnResponse::Hang);
    rig.send(BridgeCommand::SayToCaptain {
        text: "one more tweak".into(),
    })
    .await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 })
        .await;
    rig.wait_for("executing", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;
    // The launched plan is rev 1's ("keep"), regardless of the in-flight turn.
    let real = deps
        .recorded_missions
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(real.workstreams[0].slug, "keep");
    // No PlanProposed with revision 2 ever surfaced.
    assert!(
        !rig.collected
            .iter()
            .any(|e| matches!(e, BridgeEvent::PlanProposed { revision: 2, .. }))
    );
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn conference_turn_cap_rejects_further_turns_but_allows_approval() {
    let deps = MockDeps::new();
    deps.set_plan(&[("solo", &[])]);
    let mut config = BridgeConfig::default();
    config.captain.max_conference_turns = 1;
    let mut rig = spawn_rig(deps.clone(), config, None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() })
        .await;
    rig.wait_for("rev 1", |e| {
        matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })
    })
    .await;
    rig.send(BridgeCommand::SayToCaptain {
        text: "more".into(),
    })
    .await;
    rig.wait_for(
        "cap rejection",
        |e| matches!(e, BridgeEvent::ProposalRejected { reason, .. } if reason.contains("budget")),
    )
    .await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 })
        .await;
    rig.wait_for("executing", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing,
                ..
            })
        )
    })
    .await;
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn schema_garbage_gets_one_retry_then_diagnostic() {
    let deps = MockDeps::new();
    deps.push_turn(
        Station::Captain,
        TurnResponse::structured(serde_json::json!({
            "wrong": "shape"
        })),
    );
    deps.push_turn(
        Station::Captain,
        TurnResponse::structured(serde_json::json!({
            "also": "wrong"
        })),
    );
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() })
        .await;
    rig.wait_for(
        "diagnostic",
        |e| matches!(e, BridgeEvent::CaptainSays { text, .. } if text.contains("unreadable")),
    )
    .await;
    // Two turns ran: original + one corrective retry.
    rig.wait_until("two captain turns", || {
        deps.turn_log()
            .iter()
            .filter(|t| t.station == Station::Captain)
            .count()
            == 2
    })
    .await;
    // Conference is still alive: saying more works.
    rig.send(BridgeCommand::SayToCaptain {
        text: "try again".into(),
    })
    .await;
    rig.wait_for("recovered", |e| {
        matches!(e, BridgeEvent::PlanProposed { .. })
    })
    .await;
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn turn_error_keeps_conference_recoverable() {
    let deps = MockDeps::new();
    deps.push_turn(Station::Captain, TurnResponse::Err("spawn failed".into()));
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() })
        .await;
    rig.wait_for(
        "error surfaced",
        |e| matches!(e, BridgeEvent::CaptainSays { text, .. } if text.contains("did not respond")),
    )
    .await;
    rig.send(BridgeCommand::SayToCaptain {
        text: "retry".into(),
    })
    .await;
    rig.wait_for("recovered", |e| {
        matches!(e, BridgeEvent::PlanProposed { .. })
    })
    .await;
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn planning_state_emitted_at_conference_open_and_mission_recorded() {
    let deps = MockDeps::new();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain {
        text: "objective".into(),
    })
    .await;
    rig.wait_for("planning state", |e| {
        matches!(
            e,
            BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Planning,
                ..
            })
        )
    })
    .await;
    rig.wait_until("placeholder mission recorded", || {
        !deps.recorded_missions.lock().unwrap().is_empty()
    })
    .await;
    assert!(
        deps.recorded_missions.lock().unwrap()[0]
            .workstreams
            .is_empty()
    );
    rig.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// small pure helpers

#[test]
fn slugify_produces_valid_kebab_slugs() {
    assert_eq!(slugify("Fix the Bug!"), "fix-the-bug");
    assert_eq!(slugify("  weird   spacing  "), "weird-spacing");
    assert_eq!(slugify("???"), "mission");
    assert_eq!(slugify(""), "mission");
    let long = slugify(&"word ".repeat(40));
    assert!(long.len() <= 48);
    assert!(!long.ends_with('-'));
}

#[test]
fn truncate_summary_caps_length() {
    assert_eq!(truncate_summary("short", 10), "short");
    let t = truncate_summary(&"x".repeat(300), 200);
    assert_eq!(t.chars().count(), 203);
    assert!(t.ends_with("..."));
}
