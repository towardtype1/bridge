# Bridge GUI Redesign - Apple-minimal - Design

**Status:** approved direction, ready for implementation planning.
**Date:** 2026-07-06.
**Replaces the look of:** the current LCARS-amber-on-dark egui UI in `crates/bridge-app/src/ui.rs`.
**Visual reference:** the "native-pass" mockup artifact from the brainstorming session (macOS-app treatment, light + dark).

## Goal

Redesign the Bridge GUI as a minimal, clean, Apple-native macOS application: San Francisco system type, one system-blue accent, hairline separators, grouped-inset cards, tinted status pills, generous whitespace, first-class light and dark. Calm by default; loud only when something needs the user. The guiding instinct throughout is **quiet until it matters** - detail appears at the moment it is actionable and stays out of the way otherwise.

## Scope

Mostly a rendering change, but three deliberate exceptions reach into the backend. Stated honestly so the plan is sized right.

**Changes:**
- `crates/bridge-app/src/ui.rs` - rewritten around the new layout and components.
- `crates/bridge-app/src/theme.rs` - **new**: the token system as an `egui::Visuals` + a `Tokens` struct (semantic colours, spacing, radii), for both themes, with a light/dark resolver.
- `crates/bridge-app/src/state.rs` - label helpers change casing (`status_label`, `mission_state_label`: `UPPERCASE` -> `Title Case`); `WorkstreamPanel` gains `worktree_path: Option<PathBuf>` for Open-in-VS-Code; the `red_alert` field and its handling are removed. Reducer tests updated to match.
- **Red Alert removed end to end** (see below) - touches `bridge-core`, `bridge-tactical`, `bridge-stations`, `bridge-app`, config.
- **Worktree path to the GUI** (for Open-in-VS-Code) - a `bridge-core` event addition + a controller emit.

**Stays as-is:** the `AppState` reducer shape and query methods (aside from the two edits above), the rest of `BridgeEvent`/`BridgeCommand`, `main.rs` boot flow, and every non-GUI crate except where Red Alert or the worktree-path event touch them.

**Non-goals:** no new mission functionality; no plan-graph in the sidebar; no operator/telemetry maximalism; no custom window chrome (the OS title bar stays; the mockup's traffic lights are illustrative).

## Design principles

1. **One accent.** System blue marks selection, primary action, and focus only. Everything else is neutral until it carries state.
2. **Semantic colour is separate from the accent** and always paired with a label (good / warn / critical / info), never colour alone.
3. **Depth by hairline and grouped fill, not shadow.** Cards are a filled rounded container with a 1px hairline on a slightly different ground - the macOS System Settings look. No drop shadows on inline content.
4. **Whitespace is the layout.** Padding and gaps group things; avoid boxes-within-boxes.
5. **Data is monospace, prose is sans.** Timestamps, diffs, paths, hook rules, branch names are mono; everything else is SF.
6. **Both themes first-class**; every token has a light and dark value.
7. **Native patterns win.** Secondary/destructive actions live in menus, not persistent chrome. Match macOS affordances (source-list selection, ellipsis-circle menu, sentence-case labels; uppercase only for source-list section headers).
8. **No italics. No decorative glyphs.**

## Token system

Two sets resolved per active theme; in egui, an `egui::Visuals` plus a `Tokens` struct for what `Visuals` does not model (semantic colours, spacing scale, radii, pill tints).

### Colour - Light

| Token | Hex | Use |
| --- | --- | --- |
| `bg` | `#F5F5F7` | window ground |
| `surface` | `#FFFFFF` | cards, content |
| `surface_2` | `#FBFBFD` | inspector, omnibar |
| `sidebar` | `#F4F4F6` | source list |
| `text` | `#1D1D1F` | primary |
| `text_2` | `#6E6E73` | secondary |
| `text_3` | `#8E8E93` | tertiary / captions |
| `hair` | `#D9D9DE` | region separators |
| `hair_2` | `#E8E8EC` | in-card separators / card border |
| `accent` | `#0071E3` | selection (at ~14% for the sidebar fill), primary action, focus |
| `fill` | `#ECECEF` | secondary control backgrounds |
| `fill_2` | `#E3E3E8` | control hover / toggle track |
| `good` | `#1E874C` | merged / clean / allow |
| `warn` | `#9A6A00` | working / escalate / paused |
| `crit` | `#C8322A` | failed / breached / deny |
| `info` | `#0071E3` | under test / tool actions |

### Colour - Dark

| Token | Hex |
| --- | --- |
| `bg` | `#1C1C1E` |
| `surface` | `#2C2C2E` |
| `surface_2` | `#262628` |
| `sidebar` | `#232325` |
| `text` | `#F5F5F7` |
| `text_2` | `#A1A1A6` |
| `text_3` | `#8E8E93` |
| `hair` | `#3A3A3C` |
| `hair_2` | `#313134` |
| `accent` | `#0A84FF` |
| `fill` | `#3A3A3C` |
| `fill_2` | `#48484A` |
| `good` | `#30D158` |
| `warn` | `#FFA000` |
| `crit` | `#FF453A` |
| `info` | `#0A84FF` |

Pill/badge backgrounds are the semantic colour blended to ~13-15% over the surface (computed in code; egui has no `color-mix`).

### Type

System faces only - no bundled fonts. egui's default proportional face approximates SF; default monospace maps to SF Mono on macOS.

| Role | Size | Weight | Notes |
| --- | --- | --- | --- |
| Content title (workstream name) | 26 | 700 | tight tracking |
| Mission title (sidebar) | 15 | 600 | with leading state dot |
| Section header | 11 | 600 | UPPERCASE, `text_3` - source-list headers only |
| Body | 14-15 | 400 | feed, card prose |
| Item title (sidebar) | 13.5 | 500 | |
| Caption | 11-12 | 400/500 | subtitles, `text_2/3` |
| Data (mono) | 11.5-13 | 400 | timestamps, diffs, paths, rules, tabular numbers |

egui carries emphasis mainly by size and colour (its weight control is limited).

### Spacing, radius

- Spacing scale (px): 2, 4, 6, 8, 12, 16, 20, 26, 30.
- Radius: cards 13, controls/pills 8, full-round pills and toggles 999, sidebar item 8, ellipsis-menu circle 999.
- **No shadows** on inline content. The only shadow is the OS window's own.

## Layout

Single macOS window; the OS supplies the title bar. Inside, a grid with **no app toolbar**.

```
┌──────────────────────────────────────────────┐  thin strip: OS traffic lights only
├────────────┬──────────────────────┬───────────┤
│ Bridge  ⋯  │ gateway-config [pill] │ Merge     │  left: SidePanel  (sidebar ~238)
│ ● mission  │  Open in VS Code      │  queue    │  center: CentralPanel (content)
│  title     │  ┌ report card ┐      │ Guardrails│  right: SidePanel (inspector ~292)
│            │  └─────────────┘      │  47·45cl  │
│ Workstreams│  Activity             │  Escalate │
│  ● merged  │   31:02 Kobayashi …   │  Deny     │
│  ● test ◄  │                       │  Show all │
│  ● working │                       │           │
├────────────┴──────────────────────┴───────────┤
│ [ Captain | Give the Captain your objective ↑ ]│  bottom: composer
└──────────────────────────────────────────────┘
```

egui construction order (top, bottom, left, right, central):
1. Conditional banners (top): **paused** (rate-limit or budget) and **compatibility warning** only. (No Red Alert banner; it is removed.)
2. Composer (bottom).
3. Sidebar (left).
4. Inspector (right).
5. Content (central).
6. Escalation modal + merge-confirmation modal (`Window`, centered).

### Sidebar
A flex column, three parts, no footer:
- **Mission header:** "Bridge" wordmark + an **ellipsis-circle ⋯ menu** (hosts *Wind down* and *Stop*). Below, the **mission title** led by a small **state dot** - colour encodes mission state (blue executing / amber paused / green complete / red failed), a gentle pulse while executing, tooltip carries the state name. No text label.
- **Workstream source list:** section header "Workstreams" (uppercase), one row per workstream: status dot + name + status subtitle. Selected row = **soft translucent-accent fill (~14%) with normal label text** (gray when unfocused), rounded. Click sets `ui.selected`. Scrolls.

### Content
Empty state (nothing selected): **Ship's Log** - the global `logs` feed. Selected workstream:
- Title row: name (700) + status pill (semantic-tinted) + an **Open in VS Code** button (subtle `fill`, no border/shadow) right-aligned; plus an **Override** button only while `Flagged` (emits `OverrideFlagged`).
- Subtitle (mono, `text_3`): `station · branch`.
- **Battle-report card** (grouped-inset): verdict (`◇ Clean` good / `Breached` crit), meta, prose summary, stats row (diff / files / tests / suite). Latest round; older rounds collapse to a count.
- **Turn history** (compact, recent): mono rows, errors in `crit`.
- **Activity feed:** `ts` (mono) · `who` (station-coloured) · message (body, inline mono chips for paths/commands). Sticks to bottom, scrolls.

### Inspector
Two groups: **Merge queue** (numbered chip + branch + state; `Merged` in good) and **Guardrails** - the hook decisions, reshaped **exceptions-only**: a header count `N checks · M clear`, then only **denials and escalations** (semantic badge + command in mono + reason caption), and a **Show all activity** text button (accent) that reveals the full stream including routine allows.

### Composer (bottom)
An elevated rounded field addressed to the Captain (Bridge's only user-facing agent): a leading **"Captain"** tag (accent-tinted pill, no glyph), the objective text (placeholder "Give the Captain your next objective…"), a **focus ring** when focused, and a trailing **circular ↑ send** button. Enter also submits -> `StartMission`. Full width; no other buttons in the bar (Wind down/Stop moved to the mission menu).
*Note: the circular send button is the iOS Messages pattern; retained deliberately because Messages for Mac uses it. If a stricter macOS read is wanted later, replace with Return-only or a bordered "Send".*

### Modals
- **Escalation** (`Window`, centered): question, tool + input summary (mono), a live "Auto-deny in m:ss" (`crit`) from `escalation_remaining_secs`, deny-reason field, Approve (good) / Deny (crit) -> `ResolveEscalation`; "N more pending" caption.
- **Merge confirmation** (`Window`, centered): `branch -> target`, summary, diff stat (mono), Confirm (accent) / Reject (ghost) -> `ConfirmMerge`.

### Banners
- **Paused:** a `warn`-tinted top bar shown while the mission is `Paused`. For a rate-limit pause it shows the retry countdown from `rate_limit_countdown_text`; for a budget pause it names the exhausted budget and hosts the **Extend budget** action (`ExtendBudget`) - the only home for budget extension now that the toolbar is gone.
- **Compat warning:** `warn`-tinted bar with detected-vs-range text and "Proceed anyway".

## Removed: Red Alert (end to end)

Red Alert (the "escalate every tool call" mode) is removed from the product, not just hidden. The plan deletes:
- `BridgeCommand::SetRedAlert` and `BridgeEvent::RedAlert`.
- `AppState.red_alert` + its reducer arm; the header toggle and full-window red banner.
- `PolicyEngine` red-alert state, its `set_red_alert`/`red_alert` methods, the "Red Alert escalates everything" step in the decision order, and the "all MCP writes escalate under Red Alert" rule; associated tests.
- The controller's `SetRedAlert` command handling and `TacticalPort::set_red_alert` (and its mock).
- `TacticalConfig.red_alert_default` and the wiring that applies it.

Consequence, recorded intentionally: Bridge loses its on-demand max-oversight valve. Normal policy still denies dangerous calls and escalates ambiguous ones, so agents remain bounded; the user simply cannot force-escalate everything at will. Accepted.

## Added: Open in VS Code

Each workstream is a real worktree on disk. The focused workstream's header gets an **Open in VS Code** button that launches the user's editor on the worktree path.
- `bridge-core`: a new event carrying the path when a workstream is provisioned (e.g. `WorkstreamProvisioned { id, worktree_path }`), or extend the existing status event with an optional path. The controller emits it after `engineering::provision`.
- `AppState`: `WorkstreamPanel.worktree_path` set from that event.
- The GUI spawns the editor **directly** (`std::process::Command`), no controller round-trip. Editor command is `code` by default; a config key (`ui.editor_command`, default `code`) allows `cursor` / `$EDITOR`. Missing binary -> a non-blocking log entry, never a panic.
- Deferred (not in this plan): Reveal in Finder, a right-click row menu, Open terminal here.

## Motion

Minimal, gated by a reduced-motion flag (default respects the platform; when reduced, nothing animates):
- The mission state dot and any working status dot soft-pulse (~2s).
- Nothing else animates.
- Continuous repaint only while a mission is live or a countdown is showing; idle windows are static.

## egui implementation mapping

- **Theme:** `theme::apply(ctx, mode)` builds `egui::Visuals` from the token set (`panel_fill=bg`, `window_fill=surface`, `extreme_bg_color=fill`, widget fills from `fill`/`fill_2`, `selection.bg_fill` = accent-at-14%, `hyperlink_color=accent`, `warn_fg_color=warn`, `error_fg_color=crit`, `window_rounding` 13, widget rounding 8, **shadows disabled**). A `Tokens` struct carries semantic colours, spacing, radii, pill-tint helper.
- **Theme selection:** follow the OS theme (eframe exposes it); the resolver takes a `mode` so a future in-app override is trivial.
- **Fonts:** none bundled; set the `TextStyle` scale to the type table; mono via `FontId::monospace`.
- **Cards / pills / badges:** `egui::Frame` (fill, rounding, hairline `stroke`, **no shadow**); tint backgrounds computed from semantic colours.
- **Panels:** `TopBottomPanel`/`SidePanel`/`CentralPanel`; keep the existing `draw` shim that builds a root `Ui` from the `Context`.
- **Composer:** a `Frame`-wrapped `TextEdit` with an accent stroke when it has focus; a circular send button (`Button` in a rounded frame); Enter submits.
- **Open in VS Code:** button handler spawns the editor process on `panel.worktree_path`.
- **Repaint:** keep `request_repaint_after(1s)` while escalations or rate-limit are active; add it while a mission is live (for the pulse); stop when idle.

## Files touched (summary)

- New: `crates/bridge-app/src/theme.rs` (+ `mod theme;`).
- Rewrite: `crates/bridge-app/src/ui.rs`.
- Edit: `crates/bridge-app/src/state.rs` (label casing; `worktree_path`; remove `red_alert`).
- Red Alert removal across `bridge-core`, `bridge-tactical`, `bridge-stations`, config.
- Worktree-path event in `bridge-core` + controller emit in `bridge-stations`.

## Verification

- `cargo test --workspace` green (reducer/label/policy/controller tests updated for the Red Alert removal and label casing; add pill-tint and theme-resolver unit tests where pure).
- `cargo clippy --workspace --all-targets` clean.
- Manual launch in a repo: both themes render; sidebar / content / inspector / composer / modals / banners styled; selection, objective submit, Open-in-VS-Code, escalation and merge flows work; no Red Alert affordance remains.

## Open questions

None blocking. Deferred: an in-app light/dark override; Reveal in Finder + right-click row menu; bundling an SF-alternative font if egui's default is too far from SF on non-mac platforms; whether the circular send button becomes Return-only.
