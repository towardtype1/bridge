# Conversational Captain (sub-project A) - design

Date: 2026-07-06.
Status: approved direction from brainstorm; this document is the buildable spec.
Companion specs: `2026-07-06-16bit-deck-design.md` (sub-project B, independent), and a future sub-project C spec (game interaction idiom) that consumes both.

## Summary

Planning stops being a single one-shot Captain turn.
It becomes a conversation: the user and the Captain exchange messages until the Captain proposes a plan and the user explicitly approves it ("Make it so").
Nothing launches without that approval.
After launch the same conversation stays available: the Captain can answer questions and propose plan amendments as approvable diffs, while running workstreams are never disturbed.

## Goals

- Converse-until-approve planning: free-form dialogue, any number of turns, explicit approval gate before `begin_mission`.
- Mid-mission conversation with plan-amendment power: add workstreams, revise or cancel not-yet-started ones, never touch running or merged work.
- Every proposal (initial or amendment) is a structured, versioned artifact the user approves by revision id, so a stale approval can never launch the wrong plan.
- UI-agnostic protocol: commands and events only, so sub-project C can reskin the surface without touching this layer.

## Non-goals

- The RPG dialogue box and any 16-bit presentation (sub-project C).
- Full replan authority (aborting or redrafting in-flight workstreams was explicitly rejected).
- Voice, sound, or notification treatments.
- Multi-mission concurrency; one conference or mission at a time, as today.

## Current state (what changes)

`start_mission` in `crates/bridge-stations/src/controller.rs` runs exactly one Captain turn with `PlanDraft::json_schema()` forced, parses the draft, and calls `begin_mission` immediately.
There is no review step, no dialogue, and no way to talk to the Captain after launch.
`BridgeCommand::StartMission { objective }` is the only entry point.

A load-bearing discovery from grounding: session resumption is half-built, but the persistence layer is complete.
The controller passes `resume: None` on every invocation today, yet the session id is already delivered on the terminal result event (`ResultEvent::session_id`), and `ComputerPort::record_session` / `session_for` already persist sessions per (workstream, station) with a cwd column.
This spec wires the Captain onto that existing path; `bridge-engine`'s exported-but-unwired `SessionRegistry` is not needed and stays untouched.

## Design

### The conference

A "conference" is the conversation state machine that owns all Captain dialogue.
It exists in two phases with identical mechanics:

- Pre-launch: no mission is executing; the conference's purpose is to produce the first approved plan.
- Mid-mission: a mission is executing; the conference's purpose is questions and amendments.

Controller state: `Conference { mission_id, slug, session: Option<SessionId>, revision: u64, latest_proposal: Option<(u64, PlanDraft)>, turn_in_flight: bool, queued: VecDeque<String>, turns_used: u32 }`.
The existing `Planning` struct is replaced by this.
`MissionState::Planning` keeps its name and now means "conference active, nothing approved yet"; no enum change.

### Captain turn protocol

Every conference turn forces one JSON schema, `CaptainReply`:

```json
{
  "message": "string - what the Captain says to the user",
  "proposed_plan": { "workstreams": [ { "slug": "...", "title": "...", "description": "...", "depends_on": [] } ] } | null
}
```

`proposed_plan` reuses `PlanDraft` verbatim as a nested schema.
The prompt (new `prompts::captain_confer`) instructs the Captain to converse in `message` (ask clarifying questions, explain trade-offs, push back) and to include `proposed_plan` only when it is ready to propose or amend.
Amendments are not a diff DSL: the Captain always re-emits the full desired plan, and the controller computes the diff.
This keeps the schema stable across both phases and avoids inventing an operations language.

Each turn streams (`OutputFormat::StreamJson`) so the GUI can watch the Captain think, exactly as planning does today.

### Commands and events

`BridgeCommand` changes:

- `SayToCaptain { text: String }` replaces `StartMission`.
  If no conference and no mission exist, it opens a conference and the text is the objective.
  If a conference turn is in flight, the text is queued and sent as the next turn's message (queued messages are concatenated in order).
  If a mission is executing, it addresses the mid-mission conference (opening it lazily on first use).
- `ApproveProposal { revision: u64 }` approves the identified proposal.
  It is accepted only if `revision` equals the latest proposal's revision; otherwise a rejection event is emitted and nothing happens.
  Approval while a turn is in flight is legal: the proposal locks in, and the in-flight turn's eventual output is logged as conversation only (any `proposed_plan` it carries is discarded).

`BridgeEvent` additions:

- `CaptainSays { mission: MissionId, text: String }` - one per completed Captain turn, carrying `message`.
- `UserSaid { mission: MissionId, text: String }` - echo of each accepted `SayToCaptain`, so every consumer (GUI, headless log, persistence) sees one canonical transcript.
- `PlanProposed { mission: MissionId, revision: u64, plan: PlanDraft, diff: Option<PlanDiff> }` - `diff` is `None` for pre-launch proposals and present for amendments.
- `ProposalRejected { mission: MissionId, revision: u64, reason: String }` - stale approval, validation failure, or conference cap reached.

`PlanDiff` (new, `bridge-core`): `{ added: Vec<String>, removed: Vec<String>, revised: Vec<String> }`, slugs only; consumers that need detail already have both plans.

### Approval and launch

Pre-launch approval takes the plan through the existing `MissionPlan::from_draft` validation and into `begin_mission`, unchanged.
The conference survives launch: its session id and revision counter carry over to the mid-mission phase.

### Amendment validation

An amendment's re-emitted plan is diffed against the current `MissionPlan` by slug and validated:

- Locked workstreams (status anything other than `Pending`) must appear with identical title, description, and dependencies; any edit or removal rejects the amendment.
- A `Pending` workstream may be revised or removed.
- Removal is rejected if any surviving workstream depends on the removed slug.
- Added workstreams follow the same rules as initial planning (unique kebab-case slugs, acyclic graph, self-contained briefs).

A rejected amendment is not shown to the user as a dead end: the controller automatically sends the rejection reason back into the session as the next turn's message (prefixed "AMENDMENT REJECTED:"), at most twice per user message, then surfaces `ProposalRejected` and waits for the user.
On approval, the controller provisions added workstreams through the existing provisioning path, cancels removed `Pending` ones, updates revised briefs, and re-computes the ready set.
Cancellation gets an honest terminal state: a new `WorkstreamStatus::Cancelled` variant (emitted via the normal status-update event), not a repurposed `Failed`.

### Session continuity

After each successful conference turn, the controller takes `ResultEvent::session_id` from the turn outcome, stores it on the conference, and persists it via the existing `ComputerPort::record_session` under `(captain_ws, Station::Captain)` with `repo_root` as cwd.
Every subsequent conference turn passes `resume: Some(session)`.
On app restart with a resumed mission, the mid-mission conference hydrates its session through `ComputerPort::session_for`; a recorded cwd that differs from `repo_root` is treated as no session (directory-scoped resume would silently fail).
If no session can be resumed, the conference falls back to a fresh session whose first message is a generated recap: the objective, the current plan, and each workstream's status.
No engine changes are required; `bridge-engine`'s unwired `SessionRegistry` stays unused.

### Budgets and accounting

The mission row is recorded in the Ship's Computer when the conference opens (state `Planning`), not at `begin_mission`, so conference turns have a mission to attach to; `begin_mission` updates the row with the approved plan.
Every conference turn is recorded as a Captain turn, exactly like today's single planning turn.
New config key `captain.max_conference_turns` (default 24) caps total conference turns per mission across both phases; at the cap, further `SayToCaptain` commands are answered with `ProposalRejected { reason: "conference turn budget exhausted" }` while approval of the latest proposal remains available.
Mid-mission conference turns also count toward `max_total_turns`, since they consume the same subscription capacity as execution turns.

### Headless mode

`--headless-smoke` maps its objective to `SayToCaptain` and auto-approves the first `PlanProposed` by echoing back its revision.
Everything else is unchanged; the smoke run never converses further.

### Failure handling

- A failed Captain turn (spawn error, non-zero exit, `is_error` result) does not kill the conference: the controller emits `CaptainSays` with a diagnostic ("The Captain did not respond: ...") and the user may simply say something again.
- A reply that fails `CaptainReply` parsing gets one automatic corrective retry ("Your last reply did not match the required schema..."); a second failure surfaces as the diagnostic path above.
- Pre-launch, the mission is only marked `Failed` if the user issues `Shutdown` or `WindDown`; a conference with errors is otherwise always recoverable.
- Approval remains the only path to `begin_mission` or to amendment application; every other flow is conversation.

### Interim GUI (throwaway by design)

Sub-project C owns the real surface.
Until then, the existing Apple-minimal GUI gets the minimal honest rendering: the bottom composer sends `SayToCaptain` instead of `StartMission`, the captain workstream's center view renders the `UserSaid`/`CaptainSays` transcript, and `PlanProposed` renders as a card listing workstreams (with `+`/`~` diff markers when present) and a "Make it so" button that sends `ApproveProposal` with the card's revision.
No styling investment beyond existing components.

## Testing

All through the existing four-port mock harness (`TurnPort` etc.), zero subprocesses:

- Multi-turn conference: converse, propose, revise, approve; assert launch uses the approved revision.
- Stale approval: approve revision N after N+1 was proposed; assert rejection and no launch.
- Approve-during-turn: approval locks; in-flight turn's `proposed_plan` is discarded, its `message` still emitted.
- Amendment matrix: add ok; revise pending ok; edit locked rejected; remove depended-on rejected; cycle rejected; auto-retry feeds reason back and is bounded.
- Session plumbing: first turn records session; second turn resumes; cwd mismatch falls back to fresh session with recap.
- Budget cap: turn 25 rejected, approval still possible.
- Headless: objective in, auto-approved plan out, JSON lines contain the new events.
- Parse failure: one corrective retry, then diagnostic.

## Compatibility notes

`StartMission` is removed rather than aliased; `main.rs` (headless) and `ui.rs` are the only senders and both are updated in the same change.
Wire-protocol consumers (headless JSON lines) gain the four new events; `MissionState` serialization is unchanged.
