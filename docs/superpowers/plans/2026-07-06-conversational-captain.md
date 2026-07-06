# Conversational Captain (Sub-project A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Planning becomes a conversation with the Captain (converse until you approve, "Make it so"), and the same conversation stays available mid-mission to propose validated plan amendments.

**Architecture:** The controller's one-shot `Planning` state is replaced by a `Conference` state machine that runs schema-forced `CaptainReply {message, proposed_plan?}` turns against a resumable Claude session (session id from `ResultEvent::session_id`, persisted via the existing `ComputerPort::record_session`/`session_for`). Proposals are versioned; `ApproveProposal {revision}` is the only path to `begin_mission` or amendment application. Amendment validation is a pure module diffing a re-emitted full `PlanDraft` against the current `MissionPlan`.

**Tech Stack:** Rust 2024; tokio; existing four-port mock harness (`MockDeps` in `crates/bridge-stations/src/testutil.rs`, `Rig` in `crates/bridge-stations/src/controller/tests.rs`); no new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-06-conversational-captain-design.md`

## Global Constraints

- Explicit approval is the ONLY path to `begin_mission` or amendment application; every failure path leaves the conference recoverable.
- Approval must reference the latest revision; a stale `ApproveProposal` is rejected via `ProposalRejected`, never applied.
- Locked workstreams (any status other than `Pending`) can never be edited or removed by an amendment.
- `StartMission` is removed, not aliased; senders are `crates/bridge-app/src/ui.rs` and `crates/bridge-app/src/main.rs` only (plus tests).
- New config key: `captain.max_conference_turns`, default `24` (a `[captain]` section; there is no existing `CaptainConfig` - this plan introduces it).
- Session mechanism: `ResultEvent::session_id` -> `ComputerPort::record_session(captain_ws, Station::Captain, session, repo_root)`; resume via `session_for` only when the recorded cwd equals `repo_root`. NO changes to `bridge-engine`.
- All work on branch `conv-captain` off `main`. Every task ends green: `cargo test -p bridge-core -p bridge-stations -p bridge-app`; final task runs the full workspace suite + clippy + fmt.
- Commit messages: conventional commits, NO co-author lines.
- The synthetic captain workstream id is `WorkstreamId(mission_id.0)` (existing convention, `controller.rs:501`).

---

### Task 1: Core protocol types - `CaptainReply`, `PlanDiff`, `CaptainConfig`

**Files:**
- Modify: `crates/bridge-core/src/plan.rs`
- Modify: `crates/bridge-core/src/config.rs`
- Modify: `crates/bridge-core/src/lib.rs` (re-exports)
- Modify: `bridge.example.toml`

**Interfaces:**
- Produces:
  - `pub struct CaptainReply { pub message: String, pub proposed_plan: Option<PlanDraft> }` with `pub fn json_schema() -> serde_json::Value` (embeds `PlanDraft::json_schema()` under an `anyOf` with null).
  - `pub struct PlanDiff { pub added: Vec<String>, pub removed: Vec<String>, pub revised: Vec<String> }` (slugs) with `pub fn between(current: &MissionPlan, draft: &PlanDraft) -> PlanDiff` and `pub fn is_empty(&self) -> bool`. "Revised" compares title, description, and the depends_on slug set (current deps derived from `plan.edges` mapped back to slugs).
  - `pub struct CaptainConfig { pub max_conference_turns: u32 }`, `Default = 24`, wired as `BridgeConfig.captain` with `#[serde(default)]`.

- [ ] **Step 1: Write the failing tests** (append to the existing tests modules of `plan.rs` and `config.rs`)

```rust
// plan.rs tests
#[test]
fn captain_reply_schema_requires_message_and_embeds_plan() {
    let s = CaptainReply::json_schema();
    assert_eq!(s["required"], serde_json::json!(["message"]));
    let embedded = &s["properties"]["proposed_plan"]["anyOf"][0];
    assert_eq!(*embedded, PlanDraft::json_schema());
}

#[test]
fn captain_reply_parses_with_and_without_plan() {
    let with: CaptainReply = serde_json::from_value(serde_json::json!({
        "message": "Plan ready.",
        "proposed_plan": { "workstreams": [
            { "slug": "a", "title": "A", "description": "Do A", "depends_on": [] }
        ]}
    }))
    .unwrap();
    assert!(with.proposed_plan.is_some());
    let without: CaptainReply =
        serde_json::from_value(serde_json::json!({ "message": "Question?" })).unwrap();
    assert!(without.proposed_plan.is_none());
    let null_plan: CaptainReply = serde_json::from_value(
        serde_json::json!({ "message": "Hmm.", "proposed_plan": null }),
    )
    .unwrap();
    assert!(null_plan.proposed_plan.is_none());
}

#[test]
fn plan_diff_between_classifies_added_removed_revised() {
    let draft_v1 = PlanDraft {
        workstreams: vec![
            PlanDraftWorkstream { slug: "keep".into(), title: "K".into(), description: "d".into(), depends_on: vec![] },
            PlanDraftWorkstream { slug: "gone".into(), title: "G".into(), description: "d".into(), depends_on: vec![] },
            PlanDraftWorkstream { slug: "edit".into(), title: "E".into(), description: "old".into(), depends_on: vec![] },
        ],
    };
    let current = MissionPlan::from_draft(draft_v1, MissionId::new(), "m", "obj", "main").unwrap();
    let draft_v2 = PlanDraft {
        workstreams: vec![
            PlanDraftWorkstream { slug: "keep".into(), title: "K".into(), description: "d".into(), depends_on: vec![] },
            PlanDraftWorkstream { slug: "edit".into(), title: "E".into(), description: "new".into(), depends_on: vec!["keep".into()] },
            PlanDraftWorkstream { slug: "fresh".into(), title: "F".into(), description: "d".into(), depends_on: vec![] },
        ],
    };
    let diff = PlanDiff::between(&current, &draft_v2);
    assert_eq!(diff.added, vec!["fresh".to_string()]);
    assert_eq!(diff.removed, vec!["gone".to_string()]);
    assert_eq!(diff.revised, vec!["edit".to_string()]);
    assert!(!diff.is_empty());
}

// config.rs tests
#[test]
fn captain_config_defaults_and_parses() {
    assert_eq!(BridgeConfig::default().captain.max_conference_turns, 24);
    let c: BridgeConfig = toml::from_str("[captain]\nmax_conference_turns = 5\n").unwrap();
    assert_eq!(c.captain.max_conference_turns, 5);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-core captain_reply plan_diff captain_config` - Expected: FAIL to compile.

- [ ] **Step 3: Implement**

In `plan.rs` (after `PlanDraft`):

```rust
/// One Captain conference turn: conversation plus an optional full plan
/// proposal. The Captain always re-emits the whole desired plan; diffs
/// are computed controller-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptainReply {
    pub message: String,
    #[serde(default)]
    pub proposed_plan: Option<PlanDraft>,
}

impl CaptainReply {
    /// JSON Schema handed to `claude --json-schema` for every conference turn.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" },
                "proposed_plan": {
                    "anyOf": [PlanDraft::json_schema(), { "type": "null" }]
                }
            },
            "required": ["message"]
        })
    }
}

/// Slug-level difference between the current plan and a proposed draft.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlanDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub revised: Vec<String>,
}

impl PlanDiff {
    pub fn between(current: &MissionPlan, draft: &PlanDraft) -> PlanDiff {
        use std::collections::{HashMap, HashSet};
        let id_to_slug: HashMap<_, _> =
            current.workstreams.iter().map(|w| (w.id, w.slug.clone())).collect();
        let current_deps = |id| -> HashSet<String> {
            current
                .edges
                .iter()
                .filter(|(_, b)| *b == id)
                .map(|(a, _)| id_to_slug[a].clone())
                .collect()
        };
        let draft_by_slug: HashMap<_, _> =
            draft.workstreams.iter().map(|w| (w.slug.clone(), w)).collect();
        let mut diff = PlanDiff::default();
        for spec in &current.workstreams {
            match draft_by_slug.get(&spec.slug) {
                None => diff.removed.push(spec.slug.clone()),
                Some(d) => {
                    let draft_deps: HashSet<String> = d.depends_on.iter().cloned().collect();
                    if d.title != spec.title
                        || d.description != spec.description
                        || draft_deps != current_deps(spec.id)
                    {
                        diff.revised.push(spec.slug.clone());
                    }
                }
            }
        }
        let current_slugs: HashSet<_> =
            current.workstreams.iter().map(|w| w.slug.clone()).collect();
        for w in &draft.workstreams {
            if !current_slugs.contains(&w.slug) {
                diff.added.push(w.slug.clone());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.revised.is_empty()
    }
}
```

In `config.rs`: add the section (mirror `UiConfig`'s pattern) and `pub captain: CaptainConfig` with `#[serde(default)]` to `BridgeConfig`:

```rust
/// Captain conference limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptainConfig {
    /// Total Captain conference turns per mission (both phases).
    pub max_conference_turns: u32,
}

impl Default for CaptainConfig {
    fn default() -> Self {
        Self { max_conference_turns: 24 }
    }
}
```

Re-export `CaptainReply`, `PlanDiff`, `CaptainConfig` from `crates/bridge-core/src/lib.rs` alongside the existing plan/config re-exports. Document in `bridge.example.toml`:

```toml
[captain]
# Total Captain conversation turns per mission (planning + mid-mission).
max_conference_turns = 24
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-core` - Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core bridge.example.toml
git commit -m "feat(core): CaptainReply, PlanDiff, and [captain] config"
```

---

### Task 2: Events, commands, and `WorkstreamStatus::Cancelled`

**Files:**
- Modify: `crates/bridge-core/src/events.rs`
- Modify: `crates/bridge-core/src/commands.rs`
- Modify: `crates/bridge-app/src/state.rs` (apply + labels)
- Modify: `crates/bridge-app/src/ui.rs` (exhaustive status matches only; the composer changes in Task 4)
- Modify: `crates/bridge-stations/src/controller.rs` (ONLY what is needed to compile: a `BridgeCommand::SayToCaptain`/`ApproveProposal` match arm that logs-and-ignores for now; `StartMission` stays until Task 4)

**Interfaces:**
- Produces:
  - `BridgeEvent::{UserSaid, CaptainSays, PlanProposed, ProposalRejected}`:

```rust
    /// User message accepted into the Captain conference (canonical echo).
    UserSaid { mission: MissionId, text: String },
    /// One completed Captain conference turn's message.
    CaptainSays { mission: MissionId, text: String },
    /// A versioned plan proposal; `diff` is present for amendments.
    PlanProposed {
        mission: MissionId,
        revision: u64,
        plan: PlanDraft,
        diff: Option<PlanDiff>,
    },
    /// Stale approval, validation failure, or conference cap reached.
    ProposalRejected { mission: MissionId, revision: u64, reason: String },
```

  - `BridgeCommand::{SayToCaptain { text: String }, ApproveProposal { revision: u64 }}` (added; `StartMission` removed in Task 4).
  - `WorkstreamStatus::Cancelled` (new variant after `Flagged`, doc: `/// Removed from the plan by an approved amendment before it started.`).
  - `AppState` additions: `pub captain_feed: Vec<(CaptainSpeaker, String)>` (bounded by `MAX_FEED`), `pub latest_proposal: Option<ProposalCard>`, with `pub enum CaptainSpeaker { You, Captain }` and `pub struct ProposalCard { pub revision: u64, pub plan: PlanDraft, pub diff: Option<PlanDiff> }`.

- [ ] **Step 1: Write the failing tests** (in `state.rs`'s tests module)

```rust
#[test]
fn captain_conversation_events_feed_transcript_and_proposal() {
    let mut s = AppState::default();
    let mission = MissionId::new();
    s.apply(BridgeEvent::UserSaid { mission, text: "build X".into() });
    s.apply(BridgeEvent::CaptainSays { mission, text: "Aye.".into() });
    assert_eq!(
        s.captain_feed,
        vec![
            (CaptainSpeaker::You, "build X".to_string()),
            (CaptainSpeaker::Captain, "Aye.".to_string()),
        ]
    );
    let plan = PlanDraft {
        workstreams: vec![PlanDraftWorkstream {
            slug: "a".into(), title: "A".into(), description: "d".into(), depends_on: vec![],
        }],
    };
    s.apply(BridgeEvent::PlanProposed { mission, revision: 1, plan: plan.clone(), diff: None });
    assert_eq!(s.latest_proposal.as_ref().unwrap().revision, 1);
    s.apply(BridgeEvent::ProposalRejected { mission, revision: 1, reason: "stale".into() });
    assert!(matches!(s.captain_feed.last().unwrap().0, CaptainSpeaker::Captain));
    // Launch clears the card.
    s.apply(BridgeEvent::MissionStatus(MissionStatusUpdate {
        mission, state: MissionState::Executing, detail: None,
    }));
    assert!(s.latest_proposal.is_none());
}

#[test]
fn cancelled_is_terminal_for_pending_merges() {
    let mut s = AppState::default();
    let ws = WorkstreamId::new();
    s.apply(BridgeEvent::WorkstreamStatus { id: ws, status: WorkstreamStatus::Cancelled });
    assert_eq!(s.workstreams[&ws].status, Some(WorkstreamStatus::Cancelled));
    assert_eq!(status_label(&WorkstreamStatus::Cancelled), "Cancelled");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-app captain_conversation cancelled_is` - Expected: FAIL to compile.

- [ ] **Step 3: Implement**

1. `events.rs`: add the four variants (exact code above) and `Cancelled` to `WorkstreamStatus`.
2. `commands.rs`: add the two new commands with doc comments; keep `StartMission` for now.
3. `state.rs`:
   - Add `CaptainSpeaker`, `ProposalCard`, and the two `AppState` fields.
   - `apply` arms: `UserSaid` pushes `(You, text)`; `CaptainSays` pushes `(Captain, text)`; `PlanProposed` sets `latest_proposal`; `ProposalRejected` pushes `(Captain, format!("Proposal rejected: {reason}"))`; in the existing `MissionStatus` arm, clear `latest_proposal` when `update.state == MissionState::Executing`. Use `push_bounded` for the feed.
   - Extend the existing `WorkstreamStatus::Merged | Failed` retain in the `WorkstreamStatus` arm to include `Cancelled`.
   - `status_label`: add `WorkstreamStatus::Cancelled => "Cancelled"`.
4. `ui.rs`: `status_color` maps `Cancelled` to `t.text_3` (grey, terminal); `status_is_active` returns `false` for it; check every other `match` on `WorkstreamStatus` the compiler flags.
5. `controller.rs`: add compile-only arms to `handle_command`:

```rust
        BridgeCommand::SayToCaptain { .. } | BridgeCommand::ApproveProposal { .. } => {
            tracing::warn!("conversational captain not wired yet (Task 4)");
        }
```

Chase every non-exhaustive `match` the compiler reports across the workspace (`cargo build --workspace`), including `bridge-computer` if it matches on `WorkstreamStatus` for persistence (it stores statuses as serialized values, so likely no change; verify).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-core -p bridge-app -p bridge-stations -p bridge-computer` - Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat(core): conference events, commands, and Cancelled status"
```

---

### Task 3: Prompts - `captain_confer` and the conversational system prompt

**Files:**
- Modify: `crates/bridge-stations/src/prompts.rs`
- Modify: `crates/bridge-stations/src/profiles.rs` (Captain arm's `append_system_prompt`)

**Interfaces:**
- Produces:
  - `pub fn captain_confer_opening(objective: &str, repo_summary: &str) -> String` - the first conference turn: states the objective, the conversational contract (converse in `message`; include `proposed_plan` only when ready; plans follow the PlanDraft rules verbatim from the old `captain_plan` - self-contained briefs, kebab-case slugs, acyclic deps; amendments re-emit the FULL desired plan).
  - `pub fn captain_recap(objective: &str, plan: &MissionPlan, statuses: &[(String, String)]) -> String` - fresh-session fallback: objective, each workstream's slug/title/status, and the same contract.
  - `pub const PLAN_REJECTED_PREFIX: &str = "PLAN REJECTED: ";` and `pub const AMENDMENT_REJECTED_PREFIX: &str = "AMENDMENT REJECTED: ";` and `pub const SCHEMA_RETRY_MSG: &str = "Your last reply did not match the required CaptainReply schema. Reply again: converse in \"message\"; include \"proposed_plan\" only when proposing a plan."`
  - The old `captain_plan` is DELETED (with its test) once Task 4 stops calling it - deletion happens in Task 4; this task only adds.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn captain_confer_opening_carries_contract() {
    let p = captain_confer_opening("Ship the frobnicator", "main branch: main");
    assert!(p.contains("Ship the frobnicator"));
    assert!(p.contains("main branch: main"));
    assert!(p.contains("proposed_plan"));
    assert!(p.contains("kebab-case"));
    assert!(p.contains("self-contained"));
    assert!(p.contains("full desired plan"), "amendment contract");
}

#[test]
fn captain_recap_lists_workstreams_and_statuses() {
    let draft = PlanDraft {
        workstreams: vec![PlanDraftWorkstream {
            slug: "part-a".into(), title: "Part A".into(),
            description: "d".into(), depends_on: vec![],
        }],
    };
    let plan =
        MissionPlan::from_draft(draft, MissionId::new(), "m", "obj", "main").unwrap();
    let p = captain_recap("obj", &plan, &[("part-a".into(), "Working".into())]);
    assert!(p.contains("part-a"));
    assert!(p.contains("Working"));
    assert!(p.contains("obj"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-stations prompts` - Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`captain_confer_opening` (model on the existing `captain_plan` body, `prompts.rs:12-38`, keeping its plan rules verbatim):

```rust
/// Opening turn of a Captain conference. The Captain converses in
/// `message` and attaches `proposed_plan` only when ready; every proposal
/// re-emits the FULL desired plan (the controller computes diffs).
pub fn captain_confer_opening(objective: &str, repo_summary: &str) -> String {
    format!(
        "You are opening a planning conference with your commanding officer.\n\
         \n\
         ## Objective\n\
         {objective}\n\
         \n\
         ## Repository\n\
         {repo_summary}\n\
         \n\
         ## How this conversation works\n\
         Every reply must satisfy the CaptainReply JSON schema: converse in \
         \"message\" (ask clarifying questions, explain trade-offs, push back), and \
         include \"proposed_plan\" only when you are ready to propose. Nothing \
         launches until your officer explicitly approves a proposal, so do not \
         rush to one if the objective is unclear. When amending later, always \
         re-emit the full desired plan, not a delta.\n\
         \n\
         ## Plan rules (when you do propose)\n\
         - Each workstream gets a short kebab-case slug (lowercase alphanumerics and \
           hyphens; it becomes part of a git branch name), a title, and a description.\n\
         - Descriptions must be fully self-contained working briefs: the executing agent \
           sees ONLY its own description, never the objective, the other workstreams, or \
           this conversation. Include every file path, constraint and acceptance \
           criterion it needs.\n\
         - Prefer independent workstreams. Only add a depends_on entry (by slug) when one \
           workstream genuinely cannot start before another has merged.\n\
         - The dependency graph must be acyclic."
    )
}
```

`captain_recap` formats the objective, then one `- {slug} ({title}): {status}` line per workstream (statuses passed as pre-rendered strings so prompts stay `bridge-core`-only), then appends the same "How this conversation works" contract paragraph (extract it into a private `const CONTRACT: &str` shared by both fns so the wording cannot drift).

`profiles.rs` Captain arm: extend `append_system_prompt` (keep the existing first sentence) with: `" You plan in conversation with your commanding officer: discuss, question, and revise; propose a plan only through the structured proposed_plan field; never launch anything yourself."`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-stations` - Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-stations/src/prompts.rs crates/bridge-stations/src/profiles.rs
git commit -m "feat(stations): conference prompts and conversational Captain profile"
```

---

### Task 4: The pre-launch conference (happy path) + `StartMission` removal

**Files:**
- Modify: `crates/bridge-stations/src/controller.rs` (replace `Planning`/`start_mission`/`captain_done`)
- Modify: `crates/bridge-core/src/commands.rs` (delete `StartMission`)
- Modify: `crates/bridge-stations/src/testutil.rs` (Captain default response becomes a `CaptainReply`)
- Modify: `crates/bridge-stations/src/controller/tests.rs` (`Rig::start` approves the first proposal)
- Modify: `crates/bridge-app/src/ui.rs` (`bottom_composer` sends `SayToCaptain`)
- Modify: `crates/bridge-app/src/main.rs` (headless: `SayToCaptain` + auto-approve first `PlanProposed`)

**Interfaces:**
- Consumes: Tasks 1-3 (`CaptainReply`, events/commands, `captain_confer_opening`).
- Produces (controller internals later tasks build on):

```rust
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
```

  Controller field `conference: Option<Conference>` (replaces `planning: Option<Planning>`; delete `Planning`). Methods: `async fn say_to_captain(&mut self, text: String)`, `fn start_captain_turn(&mut self, message: String)`, `async fn captain_done(&mut self, result: Result<TurnOutcome, EngineError>)` (rewritten), `async fn approve_proposal(&mut self, revision: u64)`, `fn record_conference_turn(&mut self, order_id: OrderId, started_at: DateTime<Utc>, outcome: &TurnOutcome)`.

- [ ] **Step 1: Update the harness so every existing test still drives missions**

In `testutil.rs`, `default_response` Captain arm wraps the draft in a reply:

```rust
Station::Captain => TurnResponse::structured(serde_json::json!({
    "message": "Plan ready for your approval.",
    "proposed_plan": self.plan_draft.lock().unwrap().clone(),
})),
```

In `controller/tests.rs`, `Rig::start` becomes propose-then-approve:

```rust
    async fn start(&mut self, objective: &str) {
        self.send(BridgeCommand::SayToCaptain { text: objective.into() }).await;
        let proposed = self
            .wait_for("plan proposed", |e| matches!(e, BridgeEvent::PlanProposed { .. }))
            .await;
        let BridgeEvent::PlanProposed { revision, .. } = proposed else { unreachable!() };
        self.send(BridgeCommand::ApproveProposal { revision }).await;
        self.wait_for("mission Executing", |e| {
            matches!(e, BridgeEvent::MissionStatus(MissionStatusUpdate {
                state: MissionState::Executing, ..
            }))
        })
        .await;
    }
```

Update the one assertion in `full_mission_two_parallel_workstreams` that checks the Captain schema: `assert_eq!(captain_turns[0].inv.json_schema, Some(CaptainReply::json_schema()));`

`Rig::plan` currently returns `recorded_missions[0]`, which is now the empty placeholder row - every existing test that looks up slugs through it would break. Change it to the last non-empty recording:

```rust
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
```

- [ ] **Step 2: Write the new failing tests** (in `controller/tests.rs`)

```rust
#[tokio::test(start_paused = true)]
async fn conference_converses_then_proposes_then_launches_on_approval() {
    let deps = MockDeps::new();
    deps.set_plan(&[("solo", &[])]);
    // First turn: pure conversation, no plan.
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "message": "What does done look like?",
    })));
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);

    rig.send(BridgeCommand::SayToCaptain { text: "ship it".into() }).await;
    rig.wait_for("user echo", |e| {
        matches!(e, BridgeEvent::UserSaid { text, .. } if text == "ship it")
    }).await;
    rig.wait_for("captain question", |e| {
        matches!(e, BridgeEvent::CaptainSays { text, .. } if text.contains("done"))
    }).await;
    // No proposal yet; approving revision 1 must be rejected.
    rig.send(BridgeCommand::ApproveProposal { revision: 1 }).await;
    rig.wait_for("no-proposal rejection", |e| {
        matches!(e, BridgeEvent::ProposalRejected { .. })
    }).await;

    // Second exchange: default response proposes the plan.
    rig.send(BridgeCommand::SayToCaptain { text: "tests pass".into() }).await;
    let BridgeEvent::PlanProposed { revision, diff, .. } = rig
        .wait_for("proposal", |e| matches!(e, BridgeEvent::PlanProposed { .. }))
        .await
    else { unreachable!() };
    assert_eq!(revision, 1);
    assert!(diff.is_none(), "pre-launch proposal has no diff");

    rig.send(BridgeCommand::ApproveProposal { revision }).await;
    rig.wait_for("executing", |e| {
        matches!(e, BridgeEvent::MissionStatus(MissionStatusUpdate {
            state: MissionState::Executing, ..
        }))
    }).await;

    // Two conference turns ran, both resumable-schema'd; second resumed.
    let captain: Vec<_> = deps.turn_log().into_iter()
        .filter(|t| t.station == Station::Captain).collect();
    assert_eq!(captain.len(), 2);
    assert_eq!(captain[0].inv.resume, None);
    assert_eq!(captain[1].inv.resume, Some(SessionId::from("mock-session")));
    // Conference turns recorded to the computer against the mission.
    assert!(deps.recorded_turns.lock().unwrap().len() >= 2);
    // Session persisted with repo_root cwd.
    let sessions = deps.recorded_sessions.lock().unwrap();
    assert!(!sessions.is_empty());
    assert_eq!(sessions[0].1, Station::Captain);

    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn planning_state_emitted_at_conference_open_and_mission_recorded() {
    let deps = MockDeps::new();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "objective".into() }).await;
    rig.wait_for("planning state", |e| {
        matches!(e, BridgeEvent::MissionStatus(MissionStatusUpdate {
            state: MissionState::Planning, ..
        }))
    }).await;
    rig.wait_until("placeholder mission recorded", || {
        !deps.recorded_missions.lock().unwrap().is_empty()
    }).await;
    assert!(deps.recorded_missions.lock().unwrap()[0].workstreams.is_empty());
    rig.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Run to verify the new tests fail**

Run: `cargo test -p bridge-stations conference_ planning_state` - Expected: FAIL (SayToCaptain arm only logs).

- [ ] **Step 4: Implement the conference (controller)**

Replace `Planning`, `start_mission`, and `captain_done`:

1. `handle_command`: `SayToCaptain { text } => self.say_to_captain(text).await`, `ApproveProposal { revision } => self.approve_proposal(revision).await`. Delete the `StartMission` arm and the `commands.rs` variant.
2. `event_loop`'s channel-closed break checks `self.conference.is_some()` instead of `self.planning`.
3. `say_to_captain` (pre-launch phase; the mid-mission branch lands in Task 7 - for now, if `self.mission.is_some()`, log a warning and return):

```rust
    async fn say_to_captain(&mut self, text: String) {
        if self.mission.is_some() {
            tracing::warn!("mid-mission conference lands in a later task");
            return;
        }
        if self.conference.is_none() {
            let mission_id = MissionId::new();
            let slug = slugify(&text);
            self.shared.emit(BridgeEvent::MissionStatus(MissionStatusUpdate {
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
            if let Err(e) = self.shared.deps.record_mission(&placeholder, &self.shared.config) {
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
            self.shared.emit(BridgeEvent::UserSaid { mission, text: text.clone() });
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
        self.shared.emit(BridgeEvent::UserSaid { mission: conf.mission_id, text: text.clone() });
        if conf.turn_in_flight.is_some() || conf.approved {
            conf.queued.push_back(text);
        } else {
            self.start_captain_turn(text);
        }
    }
```

4. `start_captain_turn` (mirrors the old `start_mission` invocation body, `controller.rs:515-556`, with three changes: `json_schema: Some(CaptainReply::json_schema())`, `resume: conf.session.clone()`, prompt = the passed message; plus the cap):

```rust
    fn start_captain_turn(&mut self, message: String) {
        let Some(conf) = self.conference.as_mut() else { return };
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
            mcp_config: self.shared.config.linear.as_ref().map(|l| l.mcp_config_path.clone()),
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
```

5. Rewrite `captain_done`:

```rust
    async fn captain_done(&mut self, result: Result<TurnOutcome, EngineError>) {
        let Some(conf) = self.conference.as_mut() else { return };
        let Some((order_id, started_at)) = conf.turn_in_flight.take() else { return };
        let mission = conf.mission_id;

        let outcome = match result {
            Ok(o) if o.exit == ExitClass::Success
                && !o.result.as_ref().is_some_and(|r| r.is_error) => o,
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
            if let Err(e) = self.shared.deps.record_session(ws, Station::Captain, &sid, &root) {
                tracing::warn!("failed to persist captain session: {e}");
            }
        }
        self.record_conference_turn(order_id, started_at, &outcome); // &mut self: conf dropped here

        let structured = outcome.structured_output.clone().or_else(|| {
            outcome.result.as_ref().and_then(|r| r.structured_output.clone())
        });
        let parsed: Option<CaptainReply> =
            structured.and_then(|v| serde_json::from_value(v).ok());
        let Some(conf) = self.conference.as_mut() else { return }; // re-fetch
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
                    text: "The Captain's reply was unreadable twice; say something to retry.".into(),
                });
                return;
            }
        };
        self.shared.emit(BridgeEvent::CaptainSays { mission, text: reply.message.clone() });

        if let Some(draft) = reply.proposed_plan {
            if self.conference.as_ref().is_some_and(|c| c.approved) {
                tracing::info!("proposal discarded: approval already locked");
            } else {
                self.handle_proposal(draft).await; // pre-launch validation below
            }
        }

        // Flush queued user messages as one concatenated turn.
        if let Some(conf) = self.conference.as_mut() {
            if !conf.queued.is_empty() && !conf.approved && conf.turn_in_flight.is_none() {
                let msg = conf.queued.drain(..).collect::<Vec<_>>().join("\n\n");
                self.start_captain_turn(msg);
            }
        }
    }
```

6. `handle_proposal` (pre-launch arm; Task 7 adds the amendment arm):

```rust
    async fn handle_proposal(&mut self, draft: PlanDraft) {
        let Some(conf) = self.conference.as_mut() else { return };
        let mission = conf.mission_id;
        // Pre-launch: validate structurally by building the real plan.
        let base_ref = self
            .shared
            .blocking(|d| d.main_branch())
            .await
            .unwrap_or_else(|_| "main".into());
        match MissionPlan::from_draft(
            draft.clone(), mission, &conf.slug, &conf.objective, &base_ref,
        ) {
            Ok(_) => {
                conf.auto_retries = 0;
                conf.revision += 1;
                conf.latest_proposal = Some((conf.revision, draft.clone()));
                self.shared.emit(BridgeEvent::PlanProposed {
                    mission, revision: conf.revision, plan: draft, diff: None,
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
                    mission, revision, reason: e.to_string(),
                });
            }
        }
    }
```

7. `approve_proposal`:

```rust
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
                    mission, revision,
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
                        c.approved = false;
                    }
                    self.begin_mission(plan, None).await;
                }
                Err(e) => {
                    self.shared.emit(BridgeEvent::ProposalRejected {
                        mission, revision, reason: format!("invalid plan: {e}"),
                    });
                }
            }
        }
        // Mid-mission amendment application lands in Task 7.
    }
```

8. `record_conference_turn` builds a `TurnRecord` exactly like `record_turn_outcome` (`controller.rs:2037-2077`) does from an outcome (order, `WorkstreamId(mission.0)`, `Station::Captain`, session from `r.session_id`, `started_at`, `duration_ms`/`num_turns`/`is_error`/`subtype`/`total_cost_usd`/token fields from the result) but calls `deps.record_turn(conference.mission_id, &record)` directly, because `record_turn_outcome` early-returns without a `Mission`. Copy the field mapping verbatim from `record_turn_outcome` rather than calling it. Spec requirement: when `self.mission.is_some()` (mid-mission conference), also apply the same budget accounting `record_turn_outcome` performs (`m.total_turns += ...`, cost accumulation - read its tail and mirror it), so mid-mission conference turns count toward `max_total_turns`.
9. Delete `fail_planning` (a failed conference is never a failed mission; `Failed` now only comes from `Shutdown`/`WindDown` paths, which already work through `event_loop`). Delete `prompts::captain_plan` and its test.
10. `ui.rs` composer: replace the `StartMission` push with `out.push(BridgeCommand::SayToCaptain { text: state.ui.objective.trim().to_owned() });` and change the hint text to `"Hail the Captain..."`.
11. `main.rs` headless: replace the `StartMission` send with `SayToCaptain { text: objective }`; add to the event match (before the `MissionStatus` arm), with a `let mut plan_approved = false;` above the loop:

```rust
            BridgeEvent::PlanProposed { revision, .. } if !plan_approved => {
                plan_approved = true;
                wiring.commands.blocking_send(BridgeCommand::ApproveProposal {
                    revision: *revision,
                })?;
            }
```

- [ ] **Step 5: Run the full stations + app suites**

Run: `cargo test -p bridge-stations -p bridge-app` - Expected: PASS, including every pre-existing controller test via the updated `Rig::start`. Expect and fix fallout in tests that referenced `StartMission` or `PlanDraft::json_schema()` on the captain invocation.

- [ ] **Step 6: Commit**

```bash
git add crates
git commit -m "feat(stations): pre-launch Captain conference replaces one-shot planning"
```

---

### Task 5: Conference robustness - stale approval, approve-during-turn, queueing, cap

**Files:**
- Modify: `crates/bridge-stations/src/controller/tests.rs` (tests only; Task 4's implementation should already satisfy them - fix it where not)

**Interfaces:** none new; this task pins behavior.

- [ ] **Step 1: Write the tests**

```rust
#[tokio::test(start_paused = true)]
async fn stale_approval_rejected_and_does_not_launch() {
    let deps = MockDeps::new();
    deps.set_plan(&[("v1", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() }).await;
    rig.wait_for("rev 1", |e| matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })).await;
    // Second proposal supersedes.
    deps.set_plan(&[("v2", &[])]);
    rig.send(BridgeCommand::SayToCaptain { text: "actually, rename it".into() }).await;
    rig.wait_for("rev 2", |e| matches!(e, BridgeEvent::PlanProposed { revision: 2, .. })).await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 }).await;
    rig.wait_for("stale rejected", |e| matches!(
        e, BridgeEvent::ProposalRejected { revision: 1, .. }
    )).await;
    assert!(deps.recorded_missions.lock().unwrap()[0].workstreams.is_empty(),
        "nothing launched");
    rig.send(BridgeCommand::ApproveProposal { revision: 2 }).await;
    rig.wait_for("executing", |e| matches!(e, BridgeEvent::MissionStatus(
        MissionStatusUpdate { state: MissionState::Executing, .. }
    ))).await;
    assert_eq!(rig.plan().workstreams[0].slug, "v2");
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn approval_during_in_flight_turn_locks_and_discards_late_proposal() {
    let deps = MockDeps::new();
    deps.set_plan(&[("keep", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() }).await;
    rig.wait_for("rev 1", |e| matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })).await;
    // A turn that never completes keeps the conference in-flight
    // deterministically while we approve rev 1 underneath it.
    deps.push_turn(Station::Captain, TurnResponse::Hang);
    rig.send(BridgeCommand::SayToCaptain { text: "one more tweak".into() }).await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 }).await;
    rig.wait_for("executing", |e| matches!(e, BridgeEvent::MissionStatus(
        MissionStatusUpdate { state: MissionState::Executing, .. }
    ))).await;
    // The launched plan is rev 1's ("keep"), regardless of the in-flight turn.
    let real = deps.recorded_missions.lock().unwrap().last().unwrap().clone();
    assert_eq!(real.workstreams[0].slug, "keep");
    // No PlanProposed with revision 2 ever surfaced.
    assert!(!rig.collected.iter().any(|e| matches!(
        e, BridgeEvent::PlanProposed { revision: 2, .. }
    )));
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn conference_turn_cap_rejects_further_turns_but_allows_approval() {
    let deps = MockDeps::new();
    deps.set_plan(&[("solo", &[])]);
    let mut config = BridgeConfig::default();
    config.captain.max_conference_turns = 1;
    let mut rig = spawn_rig(deps.clone(), config, None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() }).await;
    rig.wait_for("rev 1", |e| matches!(e, BridgeEvent::PlanProposed { revision: 1, .. })).await;
    rig.send(BridgeCommand::SayToCaptain { text: "more".into() }).await;
    rig.wait_for("cap rejection", |e| matches!(
        e, BridgeEvent::ProposalRejected { reason, .. } if reason.contains("budget")
    )).await;
    rig.send(BridgeCommand::ApproveProposal { revision: 1 }).await;
    rig.wait_for("executing", |e| matches!(e, BridgeEvent::MissionStatus(
        MissionStatusUpdate { state: MissionState::Executing, .. }
    ))).await;
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn schema_garbage_gets_one_retry_then_diagnostic() {
    let deps = MockDeps::new();
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "wrong": "shape"
    })));
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "also": "wrong"
    })));
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() }).await;
    rig.wait_for("diagnostic", |e| matches!(
        e, BridgeEvent::CaptainSays { text, .. } if text.contains("unreadable")
    )).await;
    // Two turns ran: original + one corrective retry.
    rig.wait_until("two captain turns", || {
        deps.turn_log().iter().filter(|t| t.station == Station::Captain).count() == 2
    }).await;
    // Conference is still alive: saying more works.
    rig.send(BridgeCommand::SayToCaptain { text: "try again".into() }).await;
    rig.wait_for("recovered", |e| matches!(e, BridgeEvent::PlanProposed { .. })).await;
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn turn_error_keeps_conference_recoverable() {
    let deps = MockDeps::new();
    deps.push_turn(Station::Captain, TurnResponse::Err("spawn failed".into()));
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.send(BridgeCommand::SayToCaptain { text: "go".into() }).await;
    rig.wait_for("error surfaced", |e| matches!(
        e, BridgeEvent::CaptainSays { text, .. } if text.contains("did not respond")
    )).await;
    rig.send(BridgeCommand::SayToCaptain { text: "retry".into() }).await;
    rig.wait_for("recovered", |e| matches!(e, BridgeEvent::PlanProposed { .. })).await;
    rig.shutdown().await.unwrap();
}
```

Note on `schema_garbage_gets_one_retry_then_diagnostic`: the corrective retry consumes the second pushed response; both are wrong shapes, so the diagnostic fires. If Task 4's retry accounting differs, fix the implementation, not the test's intent.

- [ ] **Step 2: Run and fix until green**

Run: `cargo test -p bridge-stations` - Expected: PASS. Where a test fails, the bug is in Task 4's implementation; fix there.

- [ ] **Step 3: Commit**

```bash
git add crates/bridge-stations
git commit -m "test(stations): conference robustness - stale/locked approvals, cap, retries"
```

---

### Task 6: Session hydration for resumed missions (recap fallback)

**Files:**
- Modify: `crates/bridge-stations/src/controller.rs` (mid-mission conference opening reads `session_for`; this pairs with Task 7 but the hydration + recap logic is separately testable via `with_resumed_plan`)

**Interfaces:**
- Produces: `async fn open_mid_mission_conference(&mut self)` - creates a `Conference` for the running mission: session = `deps.session_for(captain_ws, Station::Captain)` filtered to `cwd == repo_root`; when no session survives, the NEXT turn's message is prefixed with `prompts::captain_recap(...)` built from `m.plan` and current statuses.

- [ ] **Step 1: Write the failing tests**

```rust
fn small_plan() -> MissionPlan {
    MissionPlan::from_draft(
        PlanDraft { workstreams: vec![PlanDraftWorkstream {
            slug: "solo".into(), title: "Solo".into(),
            description: "d".into(), depends_on: vec![],
        }]},
        MissionId::new(), "m", "obj", "main",
    ).unwrap()
}

#[tokio::test(start_paused = true)]
async fn resumed_mission_conference_recaps_when_no_session_survives() {
    let deps = MockDeps::new();
    let plan = small_plan();
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), Some(plan));
    rig.send(BridgeCommand::SayToCaptain { text: "status?".into() }).await;
    rig.wait_for("captain answers", |e| matches!(e, BridgeEvent::CaptainSays { .. })).await;
    let captain: Vec<_> = deps.turn_log().into_iter()
        .filter(|t| t.station == Station::Captain).collect();
    assert_eq!(captain.len(), 1);
    assert_eq!(captain[0].inv.resume, None, "no persisted session -> fresh");
    assert!(captain[0].inv.prompt.contains("solo"), "recap names the workstream");
    assert!(captain[0].inv.prompt.contains("status?"), "user text appended");
    rig.shutdown().await.unwrap();
}
```

Also add a `MockDeps` seam if none exists: `session_for` must be scriptable. Check `testutil.rs`'s `ComputerPort` impl - if `session_for` currently returns `Ok(None)` unconditionally, add `pub sessions_script: Mutex<HashMap<(WorkstreamId, Station), (SessionId, PathBuf)>>` consulted first, and a second test:

```rust
#[tokio::test(start_paused = true)]
async fn resumed_mission_conference_resumes_persisted_session() {
    let deps = MockDeps::new();
    let plan = small_plan();
    let captain_ws = WorkstreamId(plan.mission_id.0);
    deps.sessions_script.lock().unwrap().insert(
        (captain_ws, Station::Captain),
        (SessionId::from("old-session"), deps.repo_root()),
    );
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), Some(plan));
    rig.send(BridgeCommand::SayToCaptain { text: "status?".into() }).await;
    rig.wait_for("captain answers", |e| matches!(e, BridgeEvent::CaptainSays { .. })).await;
    let captain: Vec<_> = deps.turn_log().into_iter()
        .filter(|t| t.station == Station::Captain).collect();
    assert_eq!(captain[0].inv.resume, Some(SessionId::from("old-session")));
    assert!(!captain[0].inv.prompt.contains("## Objective"), "no recap when resuming");
    rig.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p bridge-stations resumed_mission_conference` - Expected: FAIL (Task 4's `say_to_captain` warns and returns when a mission exists).

- [ ] **Step 3: Implement**

In `say_to_captain`, replace the `self.mission.is_some()` early return with:

```rust
        if self.mission.is_some() && self.conference.is_none() {
            self.open_mid_mission_conference(text).await;
            return;
        }
```

```rust
    async fn open_mid_mission_conference(&mut self, first_text: String) {
        let Some(m) = self.mission.as_ref() else { return };
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
                    let s = m.ws.get(&w.id).map(|ws| format!("{:?}", ws.status))
                        .unwrap_or_else(|| "Unknown".into());
                    (w.slug.clone(), s)
                })
                .collect();
            Some(prompts::captain_recap(&objective, &m.plan, &statuses))
        } else {
            None
        };
        self.conference = Some(Conference {
            mission_id, slug, objective,
            session,
            revision: 0,
            latest_proposal: None,
            turn_in_flight: None,
            queued: VecDeque::new(),
            turns_used: 0,
            auto_retries: 0,
            approved: false,
        });
        self.shared.emit(BridgeEvent::UserSaid { mission: mission_id, text: first_text.clone() });
        let message = match recap {
            Some(r) => format!("{r}\n\n## Your officer says\n{first_text}"),
            None => first_text,
        };
        self.start_captain_turn(message);
    }
```

Add the `sessions_script` seam to `MockDeps` (`session_for` consults it, falling back to recorded `record_session` calls, then `Ok(None)`).

One subtlety: the conference's `revision` continues from 0 mid-mission; that is correct because pre-launch and mid-mission conferences are distinct `Conference` values, and approvals always name the revision from the `PlanProposed` event they saw.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-stations` - Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-stations
git commit -m "feat(stations): mid-mission conference opening with session hydration and recap"
```

---

### Task 7: Amendments - validation module, diff events, and application

**Files:**
- Create: `crates/bridge-stations/src/amendment.rs`
- Modify: `crates/bridge-stations/src/lib.rs` (add `mod amendment;`)
- Modify: `crates/bridge-stations/src/controller.rs` (`handle_proposal` amendment arm; `approve_proposal` application arm)
- Modify: `crates/bridge-stations/src/merge_queue.rs` (add `set_topo`)

**Interfaces:**
- Produces:
  - `pub fn validate_amendment(plan: &MissionPlan, statuses: &HashMap<WorkstreamId, WorkstreamStatus>, draft: &PlanDraft) -> Result<PlanDiff, String>` in `amendment.rs`. Rules: draft must be structurally valid (checked via `MissionPlan::from_draft` against a throwaway `MissionId`, mapping `PlanError` to its `Display`); every locked workstream (status != `Pending`) must appear in the draft with identical title, description, and depends_on slug set; removal or edit of a locked workstream is an error naming the slug; `Pending` may be revised or removed (removal with surviving dependents already fails `from_draft` as an unknown dependency); an empty diff is an error ("amendment changes nothing").
  - `MergeQueue::set_topo(&mut self, topo: Vec<WorkstreamId>)` (replaces the private `topo` field contents).
  - Controller: amendment application in `approve_proposal` when `self.mission.is_some()`.

- [ ] **Step 1: Write the failing unit tests for `amendment.rs`** (pure, no rig)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::*;
    use std::collections::HashMap;

    fn plan_and_statuses(
        specs: &[(&str, &[&str])],
        statuses: &[(&str, WorkstreamStatus)],
    ) -> (MissionPlan, HashMap<WorkstreamId, WorkstreamStatus>) {
        let draft = PlanDraft {
            workstreams: specs.iter().map(|(slug, deps)| PlanDraftWorkstream {
                slug: (*slug).into(), title: format!("T {slug}"),
                description: format!("D {slug}"),
                depends_on: deps.iter().map(|d| (*d).to_string()).collect(),
            }).collect(),
        };
        let plan = MissionPlan::from_draft(draft, MissionId::new(), "m", "o", "main").unwrap();
        let map = statuses.iter().map(|(slug, st)| {
            let id = plan.workstreams.iter().find(|w| w.slug == *slug).unwrap().id;
            (id, st.clone())
        }).collect();
        (plan, map)
    }

    fn draft(specs: &[(&str, &str, &[&str])]) -> PlanDraft {
        PlanDraft {
            workstreams: specs.iter().map(|(slug, desc, deps)| PlanDraftWorkstream {
                slug: (*slug).into(), title: format!("T {slug}"),
                description: (*desc).to_string(),
                depends_on: deps.iter().map(|d| (*d).to_string()).collect(),
            }).collect(),
        }
    }

    #[test]
    fn adding_a_workstream_is_ok() {
        let (plan, st) = plan_and_statuses(
            &[("run", &[])], &[("run", WorkstreamStatus::Working)]);
        let d = draft(&[("run", "D run", &[]), ("new", "D new", &[])]);
        let diff = validate_amendment(&plan, &st, &d).unwrap();
        assert_eq!(diff.added, vec!["new".to_string()]);
        assert!(diff.removed.is_empty() && diff.revised.is_empty());
    }

    #[test]
    fn revising_pending_is_ok_but_locked_is_rejected() {
        let (plan, st) = plan_and_statuses(
            &[("run", &[]), ("wait", &[])],
            &[("run", WorkstreamStatus::Working), ("wait", WorkstreamStatus::Pending)]);
        let ok = draft(&[("run", "D run", &[]), ("wait", "new brief", &[])]);
        assert_eq!(validate_amendment(&plan, &st, &ok).unwrap().revised, vec!["wait".to_string()]);
        let bad = draft(&[("run", "EDITED", &[]), ("wait", "D wait", &[])]);
        let err = validate_amendment(&plan, &st, &bad).unwrap_err();
        assert!(err.contains("run"), "error names the locked slug: {err}");
    }

    #[test]
    fn removing_locked_rejected_removing_pending_ok() {
        let (plan, st) = plan_and_statuses(
            &[("run", &[]), ("wait", &[])],
            &[("run", WorkstreamStatus::Merged), ("wait", WorkstreamStatus::Pending)]);
        let bad = draft(&[("wait", "D wait", &[])]);
        assert!(validate_amendment(&plan, &st, &bad).is_err(), "merged must remain");
        let ok = draft(&[("run", "D run", &[])]);
        assert_eq!(validate_amendment(&plan, &st, &ok).unwrap().removed, vec!["wait".to_string()]);
    }

    #[test]
    fn removing_pending_with_survivor_dependent_rejected() {
        let (plan, st) = plan_and_statuses(
            &[("base", &[]), ("dep", &["base"])],
            &[("base", WorkstreamStatus::Pending), ("dep", WorkstreamStatus::Pending)]);
        let bad = draft(&[("dep", "D dep", &["base"])]); // base removed, dep survives
        assert!(validate_amendment(&plan, &st, &bad).is_err());
    }

    #[test]
    fn empty_diff_rejected_and_cycles_rejected() {
        let (plan, st) = plan_and_statuses(
            &[("a", &[])], &[("a", WorkstreamStatus::Pending)]);
        let same = draft(&[("a", "D a", &[])]);
        assert!(validate_amendment(&plan, &st, &same).unwrap_err().contains("nothing"));
        let cyc = draft(&[("a", "D a", &["b"]), ("b", "D b", &["a"])]);
        assert!(validate_amendment(&plan, &st, &cyc).is_err());
    }
}
```

- [ ] **Step 2: Run to verify they fail, then implement `amendment.rs`**

Run: `cargo test -p bridge-stations amendment` - FAIL to compile, then:

```rust
//! Pure amendment validation: diff a re-emitted full PlanDraft against the
//! current MissionPlan under the lock rules. No IO, no controller state.

use bridge_core::{MissionId, MissionPlan, PlanDiff, PlanDraft, WorkstreamId, WorkstreamStatus};
use std::collections::{HashMap, HashSet};

pub fn validate_amendment(
    plan: &MissionPlan,
    statuses: &HashMap<WorkstreamId, WorkstreamStatus>,
    draft: &PlanDraft,
) -> Result<PlanDiff, String> {
    // Structural validity first (slugs, unknown deps, cycles).
    MissionPlan::from_draft(
        draft.clone(), MissionId::new(), &plan.slug, &plan.objective, "validation",
    )
    .map_err(|e| e.to_string())?;

    let id_to_slug: HashMap<_, _> =
        plan.workstreams.iter().map(|w| (w.id, w.slug.as_str())).collect();
    let draft_by_slug: HashMap<_, _> =
        draft.workstreams.iter().map(|w| (w.slug.as_str(), w)).collect();

    for spec in &plan.workstreams {
        let locked = statuses
            .get(&spec.id)
            .is_none_or(|s| !matches!(s, WorkstreamStatus::Pending));
        if !locked {
            continue;
        }
        let Some(d) = draft_by_slug.get(spec.slug.as_str()) else {
            return Err(format!(
                "workstream \"{}\" is locked (already started or finished) and cannot be removed",
                spec.slug
            ));
        };
        let current_deps: HashSet<&str> = plan
            .edges
            .iter()
            .filter(|(_, b)| *b == spec.id)
            .map(|(a, _)| id_to_slug[a])
            .collect();
        let draft_deps: HashSet<&str> = d.depends_on.iter().map(String::as_str).collect();
        if d.title != spec.title || d.description != spec.description || draft_deps != current_deps
        {
            return Err(format!(
                "workstream \"{}\" is locked (already started or finished) and cannot be edited",
                spec.slug
            ));
        }
    }

    let diff = PlanDiff::between(plan, draft);
    if diff.is_empty() {
        return Err("amendment changes nothing".into());
    }
    Ok(diff)
}
```

Run: `cargo test -p bridge-stations amendment` - PASS. Commit:

```bash
git add crates/bridge-stations/src/amendment.rs crates/bridge-stations/src/lib.rs
git commit -m "feat(stations): pure amendment validation with lock rules"
```

- [ ] **Step 3: Write the failing controller tests for amendment flow**

```rust
#[tokio::test(start_paused = true)]
async fn mid_mission_amendment_proposed_approved_and_new_workstream_starts() {
    let deps = MockDeps::new();
    deps.set_plan(&[("first", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("go").await;

    // Captain proposes adding a workstream mid-flight.
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "message": "Recommend a second workstream.",
        "proposed_plan": { "workstreams": [
            { "slug": "first", "title": "Title first", "description": "Description first", "depends_on": [] },
            { "slug": "second", "title": "T2", "description": "D2", "depends_on": [] }
        ]}
    })));
    rig.send(BridgeCommand::SayToCaptain { text: "can we also do X?".into() }).await;
    let BridgeEvent::PlanProposed { revision, diff, .. } = rig
        .wait_for("amendment proposed", |e| matches!(e, BridgeEvent::PlanProposed { .. }))
        .await
    else { unreachable!() };
    let diff = diff.expect("amendments carry a diff");
    assert_eq!(diff.added, vec!["second".to_string()]);

    rig.send(BridgeCommand::ApproveProposal { revision }).await;
    rig.wait_for("second provisioned", |e| matches!(
        e, BridgeEvent::WorkstreamStatus { status: WorkstreamStatus::Working, .. }
    )).await;
    // The amended plan was persisted.
    rig.wait_until("plan upserted", || {
        deps.recorded_missions.lock().unwrap().last().unwrap().workstreams.len() == 2
    }).await;
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn invalid_amendment_feeds_reason_back_to_captain() {
    let deps = MockDeps::new();
    deps.set_plan(&[("first", &[])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("go").await;

    // Captain tries to edit the locked (working) workstream's brief; the
    // follow-up (auto-fed rejection) turn then answers with plain text.
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "message": "Rewriting the running brief.",
        "proposed_plan": { "workstreams": [
            { "slug": "first", "title": "Title first", "description": "EDITED", "depends_on": [] }
        ]}
    })));
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "message": "Understood, standing down."
    })));
    rig.send(BridgeCommand::SayToCaptain { text: "improve the brief".into() }).await;
    rig.wait_for("captain acknowledges rejection", |e| matches!(
        e, BridgeEvent::CaptainSays { text, .. } if text.contains("standing down")
    )).await;
    // The rejection was fed back as a turn, not surfaced as ProposalRejected.
    let feedback_turn = deps.turn_log().into_iter()
        .filter(|t| t.station == Station::Captain)
        .any(|t| t.inv.prompt.starts_with("AMENDMENT REJECTED: "));
    assert!(feedback_turn);
    rig.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn amendment_cancelling_pending_workstream_emits_cancelled() {
    let deps = MockDeps::new();
    deps.set_plan(&[("base", &[]), ("later", &["base"])]);
    let mut rig = spawn_rig(deps.clone(), BridgeConfig::default(), None);
    rig.start("go").await;
    // "later" depends on "base" (unmerged), so it is still Pending.
    deps.push_turn(Station::Captain, TurnResponse::structured(serde_json::json!({
        "message": "Cutting scope: drop the follow-up.",
        "proposed_plan": { "workstreams": [
            { "slug": "base", "title": "Title base", "description": "Description base", "depends_on": [] }
        ]}
    })));
    rig.send(BridgeCommand::SayToCaptain { text: "cut scope".into() }).await;
    let BridgeEvent::PlanProposed { revision, .. } = rig
        .wait_for("cut proposed", |e| matches!(e, BridgeEvent::PlanProposed { .. }))
        .await
    else { unreachable!() };
    rig.send(BridgeCommand::ApproveProposal { revision }).await;
    rig.wait_for("cancelled", |e| matches!(
        e, BridgeEvent::WorkstreamStatus { status: WorkstreamStatus::Cancelled, .. }
    )).await;
    rig.shutdown().await.unwrap();
}
```

- [ ] **Step 4: Implement**

1. `merge_queue.rs`:

```rust
    /// Replace the topology after an approved plan amendment.
    pub fn set_topo(&mut self, topo: Vec<WorkstreamId>) {
        self.topo = topo;
    }
```

2. `handle_proposal`: branch on `self.mission.is_some()` - the amendment arm validates with `amendment::validate_amendment(&m.plan, &statuses_map, &draft)` where `statuses_map` is built from `m.ws` (`m.ws.iter().map(|(id, w)| (*id, w.status.clone())).collect()`); on `Ok(diff)` bump revision/store/emit `PlanProposed { diff: Some(diff) }`; on `Err(reason)` run the same bounded feedback loop with `prompts::AMENDMENT_REJECTED_PREFIX`.
3. `approve_proposal` amendment application (`self.mission.is_some()` arm), a new `async fn apply_amendment(&mut self, draft: PlanDraft)`:
   - Re-validate (state may have changed since proposal: a Pending workstream might have started): `validate_amendment` again; on error emit `ProposalRejected` with the reason and return.
   - Build the slug->id map: existing specs keep their ids; new slugs get `WorkstreamId::new()`.
   - Rebuild `m.plan.workstreams` (locked specs verbatim; revised Pending specs take the draft's title/description; added specs constructed with `base_ref` = existing specs' `base_ref` value, i.e. `m.plan.workstreams[0].base_ref.clone()` or the `main_branch()` fallback when the plan somehow has none) and `m.plan.edges` from the draft's `depends_on` through the map.
   - Removed Pending workstreams: `self.set_status(id, WorkstreamStatus::Cancelled)`, set `w.started = true` in `m.ws` so `start_eligible` skips them; keep the `WsState` (history), drop the spec from `m.plan.workstreams`.
   - Added workstreams: insert fresh `WsState` (copy the literal from `begin_mission`, `controller.rs:638-650`), `record_workstream_status(id, Pending)` + emit the Pending status (mirror `begin_mission`'s explicit announce).
   - `m.order = m.plan.topo_order().unwrap_or_else(|_| m.plan.workstreams.iter().map(|w| w.id).collect());` and `m.queue.set_topo(m.order.clone())`.
   - Persist: `record_mission(&m.plan, &config)` (upserts).
   - Clear `latest_proposal`/`approved` on the conference, then `self.start_eligible().await`.
   - Verify `maybe_finish` treats `Cancelled` as terminal - find its status predicate and add `Cancelled` wherever `Merged | Failed` are grouped as terminal; the compiler will NOT flag this (matches use `matches!`), so grep `maybe_finish` and read it.

- [ ] **Step 5: Run the full stations suite**

Run: `cargo test -p bridge-stations` - Expected: PASS (all new + all pre-existing).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-stations
git commit -m "feat(stations): mid-mission plan amendments with validated diffs"
```

---

### Task 8: Interim GUI - transcript, proposal card, Make it so

**Files:**
- Modify: `crates/bridge-app/src/ui.rs` (`center()` no-selection branch; composer hint already changed in Task 4)

**Interfaces:**
- Consumes: `AppState.captain_feed`, `AppState.latest_proposal` (Task 2), `BridgeCommand::ApproveProposal`.
- Produces: `fn captain_view(ui, t, state, out)` rendered by `center()` when `state.ui.selected` is `None`.

- [ ] **Step 1: Implement (no unit test - egui rendering; the routing logic was tested in Task 2)**

In `center()`, replace the bare `ships_log(ui, t, state)` fallback with:

```rust
        let Some(selected) = state.ui.selected else {
            captain_view(ui, t, state, out);
            return;
        };
```

```rust
/// Interim conversation surface: transcript, latest proposal card, ship's
/// log below. Sub-project C replaces this with the deck dialogue.
fn captain_view(ui: &mut egui::Ui, t: &Tokens, state: &AppState, out: &mut Vec<BridgeCommand>) {
    section_label(ui, t, "Captain");
    ui.add_space(8.0);
    if let Some(card) = &state.latest_proposal {
        card_frame(ui, t, |ui| {
            ui.label(
                RichText::new(format!("Proposed plan - revision {}", card.revision))
                    .color(t.text).size(13.0).strong(),
            );
            ui.add_space(6.0);
            for ws in &card.plan.workstreams {
                let marker = match &card.diff {
                    Some(d) if d.added.contains(&ws.slug) => "+",
                    Some(d) if d.revised.contains(&ws.slug) => "~",
                    _ => "-",
                };
                ui.label(
                    RichText::new(format!("{marker} {}  {}", ws.slug, ws.title))
                        .color(t.text_2).size(12.5).monospace(),
                );
            }
            if let Some(d) = &card.diff {
                for slug in &d.removed {
                    ui.label(RichText::new(format!("x {slug}  (cancelled)"))
                        .color(t.crit).size(12.5).monospace());
                }
            }
            ui.add_space(8.0);
            let btn = egui::Button::new(
                RichText::new("Make it so").color(Color32::WHITE).size(12.5),
            ).fill(t.accent);
            if ui.add(btn).clicked() {
                out.push(BridgeCommand::ApproveProposal { revision: card.revision });
            }
        });
        ui.add_space(12.0);
    }
    egui::ScrollArea::vertical()
        .id_salt("captain_feed")
        .stick_to_bottom(true)
        .max_height(ui.available_height() * 0.5)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (speaker, text) in &state.captain_feed {
                let (name, color) = match speaker {
                    CaptainSpeaker::You => ("You", t.text_2),
                    CaptainSpeaker::Captain => ("Captain", t.accent),
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(name).color(color).size(12.5).strong());
                    ui.add_space(4.0);
                    ui.label(RichText::new(text).color(t.text).size(13.5));
                });
            }
        });
    ui.add_space(12.0);
    ships_log(ui, t, state);
}
```

`card_frame` = the existing `card` helper (`ui.rs:96`); use it directly (`card(ui, t, |ui| {...})`). Note `center()` currently takes `state: &AppState` and pushes commands via `out` - `captain_view` follows the same pattern. Import `CaptainSpeaker` from `crate::state`.

- [ ] **Step 2: Build and manual smoke**

Run: `cargo build -p bridge-app && cargo test -p bridge-app` - Expected: green build; all tests pass.
Then `cargo run --release` in a scratch repo: type an objective, watch the conversation appear, revise via the composer, click "Make it so", watch the mission launch. Mid-mission: type again, see the amendment card with `+`/`~`/`x` markers.

- [ ] **Step 3: Commit**

```bash
git add crates/bridge-app
git commit -m "feat(app): interim conversation view with proposal card"
```

---

### Task 9: Headless end-to-end + workspace green

**Files:**
- Modify: whatever the sweep touches.

- [ ] **Step 1: Headless smoke against the real binary (mock-free path check)**

Run (from a scratch git repo with at least one commit; requires the Claude CLI logged in - if unavailable, skip to Step 2 and note it in the commit message):

```bash
cargo build --release
cd $(mktemp -d) && git init -q && git commit -q --allow-empty -m init
/Users/edd/Code/personal/bridge/target/release/bridge --headless-smoke "Add a CONTRIBUTING.md with a short contribution guide"
```

Expected: JSON lines including `UserSaid`, `CaptainSays`, `PlanProposed`, then the auto-approval drives `MissionStatus: Executing` through to `Complete`, exit code 0.

- [ ] **Step 2: Full workspace verification**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all green. Fix everything real; no `#[allow]` shortcuts.

- [ ] **Step 3: Commit and push**

```bash
git add -A && git commit -m "chore: conversational captain sweep - clippy, fmt, headless smoke"
git push -u origin conv-captain
```

---

## Self-Review Notes

- Spec coverage: conference concept + both phases (Tasks 4, 6, 7), `CaptainReply` schema (1, 4), commands/events incl. `UserSaid` echo (2, 4), approval-by-revision + stale rejection + approve-during-turn (4, 5), amendment rules incl. `Cancelled` and dependent-removal rejection (7), auto-feedback bounded at 2 (4's `handle_proposal`, exercised in 7), session continuity via `ResultEvent::session_id` + `record_session`/`session_for` with cwd guard (4, 6), budgets/accounting via placeholder mission row + `record_turn` (4) and cap (5), headless auto-approve (4, 9), failure handling incl. schema retry (4, 5), interim GUI (8), `StartMission` removal with both senders updated (4).
- Known parallel-work conflict: Task 8 and sub-project B's Task 7 both rewrite `center()`'s structure. Whichever branch merges second rebases; the conflict is contained to `center()`'s fallback branch (A) vs its body (B) - resolve by keeping B's deck as the body and A's `captain_view` as what `HailCaptain` shows (B's plan already routes `HailCaptain` to `selected = None`).
- Type consistency check: `Conference` fields used in Tasks 4-7 match the Task 4 definition; `validate_amendment` signature identical in Task 7's step 2 and the controller call; `PlanDiff::between` defined in Task 1, reused by `validate_amendment`.
- `record_turn_outcome` is NOT reused for conference turns (it requires `self.mission`); `record_conference_turn` copies its field mapping - flagged explicitly in Task 4 step 4.8 so the duplication is a conscious, documented choice.
