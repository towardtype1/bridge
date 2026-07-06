# 16-bit captain's deck renderer (sub-project B) - design

Date: 2026-07-06.
Status: approved direction from brainstorm; this document is the buildable spec.
Companion specs: `2026-07-06-conversational-captain-design.md` (sub-project A, independent), and a future sub-project C spec (game interaction idiom) that consumes both.
Visual reference: the approved interactive mockup at https://claude.ai/code/artifact/e0270ab6-290a-464c-93ac-76a8b9163ba8 - sprite proportions, scene composition, palettes, and status colors come from it.

## Summary

The GUI's center panel becomes a 16-bit pixel-art bridge deck rendered from live mission state.
Helm agents walk in from the turbolift when workstreams are provisioned, man consoles whose screens show workstream status at a glance, and walk out when their branch merges.
Stations are hoverable and clickable.
This is the stage for the eventual full 16-bit takeover; sub-project C later replaces the remaining Apple-minimal chrome (dialogue boxes, console-screen modals) on top of it.

## Goals

- A deterministic scene derived purely from existing `BridgeEvent`s and `AppState`; no new backend surface.
- Ambient status legibility: one glance at the deck answers "what is running, what is stuck, what merged".
- Clickable entities that route to detail views (existing panels for now, C's console screens later).
- Charm that never lies: every animation is triggered by a real state transition.

## Non-goals

- RPG dialogue boxes, pixel-themed panels and menus, escalation and merge hails (sub-project C).
- Sound (deferred entirely; revisit after C).
- Replacing the left sidebar, right inspector, or bottom composer (they stay Apple-minimal until C).
- New events or controller changes; B is strictly a consumer.

## Art direction (locked by the mockup)

- Native resolution 480x310, drawn 1:1 into a pixel buffer and blitted at the largest integer scale that fits the panel, letterboxed in space-black; nearest-neighbor filtering always.
- Default palette "Federation" (warm TNG carpet and beige hull); alternate "Dark Ops" (night bridge, cyan glow) selected by config `ui.deck_palette = "federation" | "dark-ops"`.
- Sprites are string-map pixel art in Rust source (one string per row, one char per pixel, chars keyed into a palette map), ported from the mockup's JS - reviewable in diffs, palette-swappable, no binary assets.
- Crew sprites are 12x14 back view with two walk frames; uniform colors by station (command red for the Captain, gold for Helm and Tactical); the Kobayashi tester is never visible, only its hazard-striped chamber door.
- Scene furniture: viewscreen with drifting starfield, captain's platform and chair, arc of helm consoles, tactical rail, ship's computer and comms consoles, turbolift doors that slide open for arrivals and departures, Kobayashi chamber door with pulsing red glow while a test runs.

## Architecture

New module family in `bridge-app`: `src/deck/`.

- `scene.rs` - pure state and transitions; no egui, no time source of its own (caller passes `now`). Unit-testable.
- `sprites.rs` - sprite string maps, palette definitions, status color tables. Data only.
- `render.rs` - `SceneState -> egui::ColorImage`, texture upload, integer-scale blit, repaint scheduling.
- `input.rs` - hit-testing in native coordinates, hover metadata, `DeckAction` mapping.

`ui.rs`'s `center()` renders the deck instead of the current workstream detail; everything else in `ui.rs` is untouched.

### Data flow

`AppState` is already fed by the event bus.
Each frame, `SceneState::sync(&AppState, now)` reconciles:

- Declarative state (which consoles exist, their status colors, mission liveness) is recomputed from `AppState` every frame; the deck can never drift from truth.
- Transient animations are edge-triggered by diffing against the previous sync: a workstream appearing (or `WorkstreamProvisioned` arriving) spawns an agent on a walk-in path; a transition to `Merged` sends the agent on a walk-out path and dims the console after departure; a transition to `Breached` raises a 3-second alarm marker over the agent; any `UnderTest` workstream pulses the Kobayashi door; a hook denial blinks the tactical rail.

Console assignment: workstreams map to consoles in plan order.
The arc lays out up to 8 consoles; beyond 8 (config default `max_concurrent` is 3, so this is rare) the viewscreen HUD shows a `+N` overflow chip and the newest workstreams share the last console slot.

### Status mapping (console screen + LED)

| WorkstreamStatus | Console |
| --- | --- |
| Pending | dim, idle screen |
| Working | amber, scrolling activity lines |
| UnderTest { .. } | cyan sweep (Kobayashi attacking) |
| Breached { .. } | red flash + agent alarm marker |
| ReadyToMerge / InMergeQueue | steady green pulse |
| Rebasing / ConflictFix | amber with warning tick |
| Merged | solid green, then agent departs and console dims |
| Failed { .. } / Flagged | steady red, no flash (needs the user, not urgency) |
| Cancelled (added by sub-project A) | console reverts to unassigned; agent departs if one was posted |

### Animation and time

All motion is time-based (native px/s, not per-frame), driven by `ctx.request_repaint_after`: ~16ms while any agent is walking or an alarm is active, 250ms otherwise (blink and pulse cadence).
Walk speed ~34 native px/s on rectilinear waypoint paths (turbolift, corridor row, console), two-frame walk cycle.
Config `ui.reduce_motion` (default false) places agents instantly, disables flicker and starfield drift, and keeps only state-color changes.

### Interaction (interim, until C)

Hovering any entity shows an egui tooltip: station name plus a one-line status hint.
Clicks emit a `DeckAction` handled in `ui.rs`:

- Captain or chair: selects the captain workstream (whose center content is A's transcript; until A lands, the current mission title view). Because the deck occupies the center, the transcript opens as an egui window over the deck; C replaces this with the dialogue box.
- Helm console: selects that workstream and opens the existing detail content as an egui window over the deck.
- Tactical rail: opens the existing guardrails content as a window.
- Kobayashi door: opens the latest battle-report card as a window.
- Ship's computer: opens the mission archive summary (existing data, plain list).
- Turbolift and viewscreen: tooltip only.

These windows are deliberately unthemed (stock egui); they are C's raw material, not a design statement.

### Rendering and performance

One `egui::ColorImage` (480x310 RGBA, ~595 KB) rebuilt only on animation frames or state changes, uploaded with `TextureOptions::NEAREST`.
Integer scale factor chosen per frame from the available rect (min 1x); the buffer rebuild is plain `fillRect`-style rect and pixel writes, no allocation in the hot path beyond the reused buffer.
egui 0.35 notes from the previous redesign apply (unified `Panel`, `CornerRadius::same`, `Shadow::NONE`); all deck-specific egui usage stays inside `render.rs`.

## Testing

- `scene.rs` unit tests with a fake clock: provision spawns a walk-in whose path ends at the assigned console; merge triggers walk-out and console release; breach raises and expires the alarm; status table maps every `WorkstreamStatus` variant; overflow beyond 8 consoles produces the HUD chip; reduce-motion places instantly.
- `input.rs` unit tests: every entity's hit-box resolves to the right `DeckAction`; boxes do not overlap ambiguously at native resolution.
- Render smoke test: a populated scene renders into a `ColorImage` of the exact native dimensions without panicking (headless, no GPU).
- Manual pixel pass in both palettes before merge (same standard as the Apple-minimal rewrite demanded).

## Migration

B replaces only `center()`.
Sidebar, inspector, composer, banners, and all commands and events are untouched.
When C lands, the egui windows above are progressively replaced by pixel-idiom surfaces, and the remaining Apple-minimal chrome retires with them.

### Addendum (2026-07-06, user direction during execution)

The deck fills the entire window within sub-project B - the staged "panels around the deck" interim is dropped.
The left sidebar and right inspector are removed in B; their content stays reachable deck-natively: workstreams via console clicks, guardrails via the tactical rail window, and the viewscreen becomes clickable, opening a mission-status window (mission title and state, merge queue, budget).
The bottom composer and the paused/compat banners remain the only non-deck chrome until C, because the composer is the sole conversation input surface before C's dialogue box exists.
When the A and B branches merge, A's interim captain view renders inside a window opened by `HailCaptain` over the full-page deck (it no longer has a panel fallback to live in).
