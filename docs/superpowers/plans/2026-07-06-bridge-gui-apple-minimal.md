# Bridge GUI Redesign (Apple-minimal) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the Bridge GUI as an Apple-native, minimal macOS app (system type, one system-blue accent, hairlines, grouped-inset cards, tinted status pills, light + dark), remove the Red Alert feature end to end, and add an Open-in-VS-Code button on each workstream.

**Architecture:** The change is mostly confined to `crates/bridge-app` (new `theme.rs`, rewritten `ui.rs`, edited `state.rs`), plus three deliberate backend reaches: removing Red Alert across `bridge-core`/`bridge-tactical`/`bridge-stations`/config, adding a `WorkstreamProvisioned` event so the GUI learns each worktree's path, and adding a `UiConfig.editor_command`. The `AppState` reducer, the event/command bus, and all non-GUI crates are otherwise untouched.

**Tech Stack:** Rust 2024, eframe/egui 0.35, serde, tokio (existing).

**Full design reference:** `docs/superpowers/specs/2026-07-06-bridge-gui-apple-minimal-design.md` (colour tables, layout diagram, component specs, egui mapping). This plan implements that spec; consult it for any visual detail not repeated here.

## Global Constraints

- No em dashes in any file; plain `-` only.
- No italics and no decorative glyphs anywhere in the UI.
- Rendering code is not unit-tested for appearance; its gate is `cargo check`/`clippy` clean plus a manual launch checklist. Pure logic (reducers, token/tint helpers, config) is TDD.
- egui has no `color-mix`; compute tinted colours in Rust.
- No drop shadows on inline content; the only shadow is the OS window's.
- Accent is system blue: `#0071E3` light, `#0A84FF` dark.
- After each task, `cargo test --workspace` and `cargo clippy --workspace --all-targets` must be clean before moving on.
- Commit after each task with a conventional-commit message; do not add Claude as co-author.

---

## File Structure

- `crates/bridge-core/src/commands.rs` - remove `SetRedAlert`.
- `crates/bridge-core/src/events.rs` - remove `BridgeEvent::RedAlert` and `DecisionSource::RedAlert`; add `BridgeEvent::WorkstreamProvisioned`.
- `crates/bridge-core/src/config.rs` - remove `TacticalConfig.red_alert_default`; add `UiConfig { editor_command }` on `BridgeConfig`.
- `crates/bridge-tactical/src/policy.rs` - remove red-alert state, methods, decision-order step, tests.
- `crates/bridge-tactical/src/server.rs` - remove the `red_alert` rule -> `DecisionSource::RedAlert` mapping.
- `crates/bridge-stations/src/ports.rs` - remove `TacticalPort::set_red_alert`.
- `crates/bridge-stations/src/controller.rs` - remove the `SetRedAlert` command arm; emit `WorkstreamProvisioned` at provisioning.
- `crates/bridge-stations/src/testutil.rs` - remove `red_alert_calls` + `set_red_alert`.
- `crates/bridge-stations/src/controller/tests.rs` - remove the red-alert command test.
- `crates/bridge-app/src/state.rs` - remove `red_alert` field/arm/test; add `worktree_path` + reducer arm; Title-case the label helpers.
- `crates/bridge-app/src/theme.rs` - NEW: tokens, `Visuals` resolver, pill-tint helper.
- `crates/bridge-app/src/ui.rs` - REWRITE to the new layout.
- `crates/bridge-app/src/wiring.rs` - remove `red_alert_default` application.
- `crates/bridge-app/src/main.rs` - `mod theme;`; drop the Flagged-driven WindDown only if it referenced red alert (it does not); no other change.
- `bridge.example.toml` - remove `red_alert_default`; add `[ui] editor_command`.

---

## Task 1: Remove Red Alert end to end

**Files:**
- Modify: `crates/bridge-core/src/commands.rs`, `crates/bridge-core/src/events.rs`, `crates/bridge-core/src/config.rs`
- Modify: `crates/bridge-tactical/src/policy.rs`, `crates/bridge-tactical/src/server.rs`
- Modify: `crates/bridge-stations/src/ports.rs`, `crates/bridge-stations/src/controller.rs`, `crates/bridge-stations/src/testutil.rs`, `crates/bridge-stations/src/controller/tests.rs`
- Modify: `crates/bridge-app/src/state.rs`, `crates/bridge-app/src/ui.rs`, `crates/bridge-app/src/wiring.rs`
- Modify: `bridge.example.toml`

**Interfaces:**
- Produces: a `BridgeCommand` enum with no `SetRedAlert`; a `BridgeEvent` enum with no `RedAlert`; `DecisionSource` with no `RedAlert`; `TacticalConfig` with no `red_alert_default`; `TacticalPort` with no `set_red_alert`. Later tasks must not reference any of these.

This is one atomic task: removing an enum variant breaks every consumer until all are updated, so the deliverable is "workspace compiles and all tests pass with zero red-alert symbols remaining."

- [ ] **Step 1: Delete the tests that assert Red Alert behavior**

In `crates/bridge-tactical/src/policy.rs`, delete these three test fns entirely: `red_alert_escalates_everything_not_denied`, `red_alert_clears`, `red_alert_default_comes_from_config` (around lines 1672-1710). In any other test that calls `eng.set_red_alert(...)` (e.g. near line 1748), remove those calls and any assertion depending on them; if a test's whole point was red alert, delete it.

In `crates/bridge-stations/src/controller/tests.rs`, delete the test `red_alert_command_reaches_tactical_and_the_bus` (around lines 1050-1058).

In `crates/bridge-app/src/state.rs`, delete the test `red_alert_toggles` (around lines 640-646).

- [ ] **Step 2: Remove from `bridge-core`**

`crates/bridge-core/src/commands.rs`: delete the `SetRedAlert(bool),` variant (line ~23).

`crates/bridge-core/src/events.rs`: delete the `RedAlert { active: bool },` variant from `BridgeEvent` (around line 52), and delete `RedAlert,` from the `DecisionSource` enum (line ~180).

`crates/bridge-core/src/config.rs`: in `TacticalConfig` delete the field `pub red_alert_default: bool,` (line ~151) and the `red_alert_default: false,` line in its `Default` impl (line ~178).

- [ ] **Step 3: Run core tests to see the downstream break**

Run: `cargo build -p bridge-core`
Expected: PASS (core compiles on its own).
Run: `cargo build -p bridge-tactical`
Expected: FAIL - references to `red_alert`/`RULE_RED_ALERT`/`DecisionSource::RedAlert` no longer resolve. This confirms the touch points to fix next.

- [ ] **Step 4: Remove from `bridge-tactical`**

`crates/bridge-tactical/src/policy.rs`:
- Delete `const RULE_RED_ALERT: &str = "red_alert";` (line ~27).
- Delete the field `red_alert: AtomicBool,` from `PolicyEngine` (line ~177).
- In `PolicyEngine::new`, delete the `red_alert: AtomicBool::new(config.red_alert_default),` initializer (line ~198).
- Delete the methods `set_red_alert` and `red_alert` (lines ~220-225).
- In `decide`, replace the red-alert wrapper (lines ~277-291) so it simply returns the adjudication:

```rust
        if payload.hook_event_name != "PreToolUse" {
            return PolicyVerdict::Pass { suspicious: false };
        }

        self.adjudicate_pre_tool_use(&ctx, payload)
```

- Remove the now-unused `use std::sync::atomic::AtomicBool;` (and `Ordering` if it becomes unused - check; keep it if other atomics remain).
- In the `decide` doc comment, delete the `- RED ALERT: anything not already denied escalates.` bullet, and in the `decide_mcp`/adjudicate doc comment delete the clause `; under Red Alert all MCP writes escalate` (line ~249).

`crates/bridge-tactical/src/server.rs`: in the rule-to-source mapping (lines ~388-397) delete the branch:

```rust
    } else if rule.starts_with("red_alert") {
        DecisionSource::RedAlert
```

so the chain goes straight from the `config.`/`prime.` branches to the final `else`.

- [ ] **Step 5: Remove from `bridge-stations`**

`crates/bridge-stations/src/ports.rs`: delete `fn set_red_alert(&self, active: bool);` from the `TacticalPort` trait (line ~69) and the `LiveDeps` impl `fn set_red_alert(&self, active: bool) { self.policy.set_red_alert(active); }` (lines ~210-211).

`crates/bridge-stations/src/controller.rs`: delete the `BridgeCommand::SetRedAlert(active) => { ... }` match arm (lines ~416-418).

`crates/bridge-stations/src/testutil.rs`: delete the field `pub red_alert_calls: Mutex<Vec<bool>>,` (line ~135), its initializer `red_alert_calls: Mutex::new(Vec::new()),` (line ~167), and the mock impl `fn set_red_alert(...) { ... }` (lines ~493-494).

- [ ] **Step 6: Remove from `bridge-app`**

`crates/bridge-app/src/state.rs`: delete the `pub red_alert: bool,` field from `AppState` (line ~29) and the `BridgeEvent::RedAlert { active } => self.red_alert = active,` arm in `apply` (line ~131).

`crates/bridge-app/src/ui.rs`: delete the `red_alert_banner` fn (lines ~87-102) and its call (line ~60); delete the Red Alert toggle block in `header` (lines ~121-139). (This file is fully rewritten in Task 5; this is the minimal edit to keep it compiling now.)

`crates/bridge-app/src/wiring.rs`: delete:

```rust
    if config.tactical.red_alert_default {
        policy.set_red_alert(true);
    }
```
(lines ~53-54).

- [ ] **Step 7: Remove from config docs**

`bridge.example.toml`: delete the `red_alert_default = false` line (line ~59) and its explanatory comment lines directly above it.

- [ ] **Step 8: Verify the whole workspace is clean**

Run: `cargo test --workspace`
Expected: PASS, all crates.
Run: `cargo clippy --workspace --all-targets`
Expected: no warnings.
Run: `grep -rn 'red_alert\|RedAlert\|SetRedAlert' crates/ bridge.example.toml`
Expected: no matches.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat!: remove Red Alert end to end

Drops the escalate-every-tool-call mode: BridgeCommand::SetRedAlert,
BridgeEvent::RedAlert, DecisionSource::RedAlert, the PolicyEngine red
alert state and decision-order step, TacticalPort::set_red_alert,
TacticalConfig.red_alert_default, and the GUI banner/toggle. Normal
policy still denies dangerous calls and escalates ambiguous ones."
```

---

## Task 2: WorkstreamProvisioned event -> GUI worktree path

**Files:**
- Modify: `crates/bridge-core/src/events.rs`
- Modify: `crates/bridge-stations/src/controller.rs`
- Modify: `crates/bridge-app/src/state.rs`

**Interfaces:**
- Produces: `BridgeEvent::WorkstreamProvisioned { id: WorkstreamId, worktree_path: PathBuf }`; `AppState::WorkstreamPanel.worktree_path: Option<PathBuf>`. Task 5 reads `panel.worktree_path` to enable the Open-in-VS-Code button.

- [ ] **Step 1: Write the failing reducer test**

In `crates/bridge-app/src/state.rs` tests module, add:

```rust
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
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p bridge-app workstream_provisioned_sets_path`
Expected: FAIL to compile - `BridgeEvent::WorkstreamProvisioned` and `worktree_path` do not exist yet.

- [ ] **Step 3: Add the event to `bridge-core`**

In `crates/bridge-core/src/events.rs`, add `use std::path::PathBuf;` if not present, and add a variant to `BridgeEvent`:

```rust
    /// A workstream's worktree was created on disk; carries its path so the
    /// GUI can offer "Open in editor".
    WorkstreamProvisioned {
        id: WorkstreamId,
        worktree_path: PathBuf,
    },
```

- [ ] **Step 4: Add the field + reducer arm in `state.rs`**

In `WorkstreamPanel` add `pub worktree_path: Option<PathBuf>,` (add `use std::path::PathBuf;` to the file's imports). In `AppState::apply`, add:

```rust
            BridgeEvent::WorkstreamProvisioned { id, worktree_path } => {
                self.panel_mut(id).worktree_path = Some(worktree_path);
            }
```

- [ ] **Step 5: Run the reducer test**

Run: `cargo test -p bridge-app workstream_provisioned_sets_path`
Expected: PASS.

- [ ] **Step 6: Emit the event from the controller**

In `crates/bridge-stations/src/controller.rs`, in `dispatch_helm`, at the provisioning success arm (line ~945 where `w.provisioned = Some(p);`), emit the event with the handle path. Replace:

```rust
                Ok(p) => {
                    if let Some(m) = self.mission.as_mut()
                        && let Some(w) = m.ws.get_mut(&ws)
                    {
                        w.provisioned = Some(p);
                    }
                }
```

with:

```rust
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
```

- [ ] **Step 7: Add a controller test that the event is emitted**

In `crates/bridge-stations/src/controller/tests.rs`, add a test that starts a mission, waits for a Helm turn on a workstream, and asserts a `WorkstreamProvisioned` event arrived for it:

```rust
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
```

Note: `MockDeps::create_worktree` returns a handle under `deps.root/<mission>/<ws>`; confirm the mission slug substring matches (adjust the `contains(...)` to the mock's actual path if needed by reading `testutil.rs` `create_worktree`).

- [ ] **Step 8: Verify**

Run: `cargo test -p bridge-core -p bridge-stations -p bridge-app`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: emit WorkstreamProvisioned with worktree path to the GUI"
```

---

## Task 3: Backend prep - editor config + Title-case labels

**Files:**
- Modify: `crates/bridge-core/src/config.rs`
- Modify: `bridge.example.toml`
- Modify: `crates/bridge-app/src/state.rs`

**Interfaces:**
- Produces: `BridgeConfig.ui: UiConfig` with `pub editor_command: String` (default `"code"`); `status_label`/`mission_state_label` returning Title Case strings. Task 5 uses `cfg.ui.editor_command` and the Title-case labels.

- [ ] **Step 1: Write the failing config test**

In `crates/bridge-core/src/config.rs` tests, add:

```rust
    #[test]
    fn ui_editor_command_defaults_to_code() {
        let cfg = BridgeConfig::default();
        assert_eq!(cfg.ui.editor_command, "code");
        let parsed: BridgeConfig =
            toml::from_str("[ui]\neditor_command = \"cursor\"\n").unwrap();
        assert_eq!(parsed.ui.editor_command, "cursor");
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p bridge-core ui_editor_command_defaults_to_code`
Expected: FAIL - `BridgeConfig` has no `ui` field.

- [ ] **Step 3: Add `UiConfig`**

In `crates/bridge-core/src/config.rs`, add a field to `BridgeConfig`:

```rust
    #[serde(default)]
    pub ui: UiConfig,
```

and the struct:

```rust
/// GUI preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Command used to open a worktree in an editor, e.g. "code", "cursor".
    pub editor_command: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            editor_command: "code".into(),
        }
    }
}
```

Export it from `bridge-core` lib.rs `pub use config::{..., UiConfig};` (add to the existing config re-export list).

- [ ] **Step 4: Run the config test**

Run: `cargo test -p bridge-core ui_editor_command_defaults_to_code`
Expected: PASS.

- [ ] **Step 5: Document it in the example config**

In `bridge.example.toml`, add near the end:

```toml
[ui]
# Command used by the "Open in VS Code" button to open a worktree.
# e.g. "code", "cursor". Must be on PATH.
editor_command = "code"
```

- [ ] **Step 6: Update the label-casing tests (they currently assert UPPERCASE)**

In `crates/bridge-app/src/state.rs` tests, change `status_labels_are_stable` and `mission_state_labels` to expect Title Case, e.g.:

```rust
    #[test]
    fn status_labels_are_stable() {
        assert_eq!(status_label(&WorkstreamStatus::Pending), "Pending");
        assert_eq!(status_label(&WorkstreamStatus::UnderTest { round: 2 }), "Under test - round 2");
        assert_eq!(status_label(&WorkstreamStatus::Breached { round: 3 }), "Breached - round 3");
        assert_eq!(status_label(&WorkstreamStatus::Flagged), "Flagged");
        assert_eq!(status_label(&WorkstreamStatus::Failed { reason: "x".into() }), "Failed");
    }

    #[test]
    fn mission_state_labels() {
        assert_eq!(mission_state_label(&MissionState::Planning), "Planning");
        assert_eq!(
            mission_state_label(&MissionState::Paused {
                reason: PauseReason::BudgetExhausted { which: "max_total_turns".into() }
            }),
            "Paused - budget exhausted (max_total_turns)"
        );
        assert_eq!(
            mission_state_label(&MissionState::Failed { reason: "engine down".into() }),
            "Failed - engine down"
        );
    }
```

- [ ] **Step 7: Run to confirm they fail**

Run: `cargo test -p bridge-app status_labels_are_stable mission_state_labels`
Expected: FAIL (current code returns UPPERCASE).

- [ ] **Step 8: Title-case the label helpers**

In `crates/bridge-app/src/state.rs`, rewrite `status_label` and `mission_state_label` and `pause_reason_text` to Title/sentence case:

```rust
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
    }
}

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
```

Leave `pause_reason_text` as-is (already lowercase sentence text).

- [ ] **Step 9: Run label tests + workspace**

Run: `cargo test -p bridge-core -p bridge-app`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: add ui.editor_command config and Title-case GUI labels"
```

---

## Task 4: theme.rs - tokens, Visuals resolver, tint helper

**Files:**
- Create: `crates/bridge-app/src/theme.rs`
- Modify: `crates/bridge-app/src/main.rs` (add `mod theme;`)

**Interfaces:**
- Produces:
  - `pub enum ThemeMode { Light, Dark }`
  - `pub struct Tokens { ... }` with public colour fields (`bg, surface, surface_2, sidebar, text, text_2, text_3, hair, hair_2, accent, fill, fill_2, good, warn, crit, info` as `egui::Color32`) plus `pub fn tokens(mode: ThemeMode) -> Tokens`.
  - `pub fn tint(base: egui::Color32, over: egui::Color32, pct: f32) -> egui::Color32` - blend `base` at `pct` over `over`.
  - `pub fn visuals(mode: ThemeMode) -> egui::Visuals` - full `Visuals` with shadows disabled, accent selection, radii.
  Task 5 calls `theme::tokens(mode)`, `theme::visuals(mode)`, and `theme::tint(...)`.

- [ ] **Step 1: Write failing tests for the token resolver and tint**

Create `crates/bridge-app/src/theme.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    #[test]
    fn tokens_differ_by_mode() {
        assert_eq!(tokens(ThemeMode::Light).accent, Color32::from_rgb(0x00, 0x71, 0xE3));
        assert_eq!(tokens(ThemeMode::Dark).accent, Color32::from_rgb(0x0A, 0x84, 0xFF));
        assert_ne!(tokens(ThemeMode::Light).bg, tokens(ThemeMode::Dark).bg);
    }

    #[test]
    fn tint_blends_toward_base() {
        // 0% -> the ground; 100% -> the base colour.
        let base = Color32::from_rgb(200, 0, 0);
        let over = Color32::from_rgb(0, 0, 0);
        assert_eq!(tint(base, over, 0.0), over);
        assert_eq!(tint(base, over, 1.0), base);
        let mid = tint(base, over, 0.5);
        assert_eq!(mid, Color32::from_rgb(100, 0, 0));
    }

    #[test]
    fn visuals_have_no_window_shadow() {
        let v = visuals(ThemeMode::Light);
        assert_eq!(v.window_shadow.blur, 0);
        assert_eq!(v.popup_shadow.blur, 0);
    }
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p bridge-app --lib theme`
Expected: FAIL - `tokens`/`tint`/`visuals`/`ThemeMode` undefined.

- [ ] **Step 3: Implement `theme.rs`**

Prepend to `crates/bridge-app/src/theme.rs` (above the test module):

```rust
//! Apple-minimal design tokens and the egui theme they produce.
//! One source of truth for both light and dark; see the design spec's
//! colour tables.

use eframe::egui::{self, Color32};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

/// Resolved palette for one theme. Semantic colours (good/warn/crit/info)
/// are separate from the accent and always used with a label.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_2: Color32,
    pub sidebar: Color32,
    pub text: Color32,
    pub text_2: Color32,
    pub text_3: Color32,
    pub hair: Color32,
    pub hair_2: Color32,
    pub accent: Color32,
    pub fill: Color32,
    pub fill_2: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub crit: Color32,
    pub info: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn tokens(mode: ThemeMode) -> Tokens {
    match mode {
        ThemeMode::Light => Tokens {
            bg: rgb(0xF5, 0xF5, 0xF7),
            surface: rgb(0xFF, 0xFF, 0xFF),
            surface_2: rgb(0xFB, 0xFB, 0xFD),
            sidebar: rgb(0xF4, 0xF4, 0xF6),
            text: rgb(0x1D, 0x1D, 0x1F),
            text_2: rgb(0x6E, 0x6E, 0x73),
            text_3: rgb(0x8E, 0x8E, 0x93),
            hair: rgb(0xD9, 0xD9, 0xDE),
            hair_2: rgb(0xE8, 0xE8, 0xEC),
            accent: rgb(0x00, 0x71, 0xE3),
            fill: rgb(0xEC, 0xEC, 0xEF),
            fill_2: rgb(0xE3, 0xE3, 0xE8),
            good: rgb(0x1E, 0x87, 0x4C),
            warn: rgb(0x9A, 0x6A, 0x00),
            crit: rgb(0xC8, 0x32, 0x2A),
            info: rgb(0x00, 0x71, 0xE3),
        },
        ThemeMode::Dark => Tokens {
            bg: rgb(0x1C, 0x1C, 0x1E),
            surface: rgb(0x2C, 0x2C, 0x2E),
            surface_2: rgb(0x26, 0x26, 0x28),
            sidebar: rgb(0x23, 0x23, 0x25),
            text: rgb(0xF5, 0xF5, 0xF7),
            text_2: rgb(0xA1, 0xA1, 0xA6),
            text_3: rgb(0x8E, 0x8E, 0x93),
            hair: rgb(0x3A, 0x3A, 0x3C),
            hair_2: rgb(0x31, 0x31, 0x34),
            accent: rgb(0x0A, 0x84, 0xFF),
            fill: rgb(0x3A, 0x3A, 0x3C),
            fill_2: rgb(0x48, 0x48, 0x4A),
            good: rgb(0x30, 0xD1, 0x58),
            warn: rgb(0xFF, 0xA0, 0x00),
            crit: rgb(0xFF, 0x45, 0x3A),
            info: rgb(0x0A, 0x84, 0xFF),
        },
    }
}

/// Blend `base` at `pct` (0..=1) over the opaque `over` ground, returning an
/// opaque colour. Used for tinted pill/badge/selection backgrounds since
/// egui has no color-mix.
pub fn tint(base: Color32, over: Color32, pct: f32) -> Color32 {
    let p = pct.clamp(0.0, 1.0);
    let mix = |b: u8, o: u8| ((b as f32) * p + (o as f32) * (1.0 - p)).round() as u8;
    Color32::from_rgb(
        mix(base.r(), over.r()),
        mix(base.g(), over.g()),
        mix(base.b(), over.b()),
    )
}

pub fn visuals(mode: ThemeMode) -> egui::Visuals {
    let t = tokens(mode);
    let mut v = match mode {
        ThemeMode::Light => egui::Visuals::light(),
        ThemeMode::Dark => egui::Visuals::dark(),
    };
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.extreme_bg_color = t.fill;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.crit;
    v.selection.bg_fill = tint(t.accent, t.sidebar, 0.14);
    v.selection.stroke = egui::Stroke::new(1.0, t.accent);
    v.widgets.noninteractive.bg_fill = t.surface;
    v.widgets.inactive.bg_fill = t.fill;
    v.widgets.hovered.bg_fill = t.fill_2;
    v.widgets.active.bg_fill = t.fill_2;
    v.window_stroke = egui::Stroke::new(1.0, t.hair);
    // No shadows on inline content or popups; the OS window casts its own.
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    let radius = egui::CornerRadius::same(8);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.inactive.corner_radius = radius;
    v.widgets.hovered.corner_radius = radius;
    v.widgets.active.corner_radius = radius;
    v.window_corner_radius = egui::CornerRadius::same(13);
    v
}
```

Note on egui 0.35 field names: `Shadow::NONE`, `CornerRadius`, `window_corner_radius`, `corner_radius` are the 0.35 spellings. If the exact identifiers differ in the pinned egui, fix them here in this one module and adjust the test's `window_shadow.blur` access accordingly - this is the only place these names appear.

- [ ] **Step 4: Register the module**

In `crates/bridge-app/src/main.rs`, add `mod theme;` alongside the existing `mod state; mod ui; mod wiring;`.

- [ ] **Step 5: Run theme tests**

Run: `cargo test -p bridge-app --lib theme`
Expected: PASS.
Run: `cargo clippy -p bridge-app --all-targets`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add Apple-minimal theme tokens and egui Visuals resolver"
```

---

## Task 5: Rewrite ui.rs to the Apple-minimal layout

**Files:**
- Rewrite: `crates/bridge-app/src/ui.rs`

**Interfaces:**
- Consumes: `theme::{ThemeMode, Tokens, tokens, tint, visuals}`; `AppState` (incl. `workstreams[..].worktree_path`, Title-case `status_label`/`mission_state_label`); `BridgeConfig.ui.editor_command` (thread it in - see Step 1); `BridgeCommand` (no `SetRedAlert`).
- Produces: the same `pub fn draw(ctx: &egui::Context, state: &mut AppState, out_commands: &mut Vec<BridgeCommand>)` entry point, plus `pub use eframe::egui;`. The signature is unchanged so `main.rs` needs no change.

This task has no unit tests (rendering); its gate is compile + clippy + the manual launch checklist in Step 12. Build it panel by panel, following the spec's Layout and Component sections. Keep each panel a small `fn` as the current file does.

- [ ] **Step 1: Decide how the editor command reaches `ui.rs`**

`draw`'s signature is fixed and has no config. Store the editor command in `AppState.ui` (the `UiInputs` struct) at startup: add `pub editor_command: String` to `UiInputs` in `state.rs` (default empty), and in `wiring.rs`/`main.rs` where `AppState` is created for the app, set it from `config.ui.editor_command`. If `AppState` is created with `Default` inside the eframe app, thread the value via the `BridgeApp` constructor and assign it once. Concretely: in `crates/bridge-app/src/main.rs` `BridgeApp::new`, after building `AppState::default()`, set `state.ui.editor_command = editor_command;` where `editor_command` is passed into `BridgeApp::new` from `wiring`/main (add the parameter). Add a reducer-independent field, so no event handling changes.

- [ ] **Step 2: Write the theme application + root of `draw`**

Replace the top of `ui.rs`. Remove the old `AMBER`/`ALERT_RED` consts and `apply_theme`. New skeleton:

```rust
use crate::state::{
    AppState, escalation_remaining_secs, format_duration_secs, mission_state_label, status_label,
};
use crate::theme::{self, ThemeMode, Tokens};
use bridge_core::{BridgeCommand, BudgetExtension, DecisionKind, UserDecision, WorkstreamId, WorkstreamStatus};
use chrono::Utc;
use eframe::egui::{self, Color32, RichText};

pub fn draw(ctx: &egui::Context, state: &mut AppState, out_commands: &mut Vec<BridgeCommand>) {
    let mode = if ctx.style().visuals.dark_mode { ThemeMode::Dark } else { ThemeMode::Light };
    ctx.set_visuals(theme::visuals(mode));
    let t = theme::tokens(mode);
    let mut root = egui::Ui::new(
        ctx.clone(),
        egui::Id::new((ctx.viewport_id(), "bridge_root_ui")),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    draw_in(&mut root, &t, state, out_commands);
}

fn draw_in(ui: &mut egui::Ui, t: &Tokens, state: &mut AppState, out: &mut Vec<BridgeCommand>) {
    let ctx = ui.ctx().clone();
    paused_banner(ui, t, state, out);
    compat_banner(ui, t, state);
    bottom_composer(ui, t, state, out);
    left_sidebar(ui, t, state, out);
    right_inspector(ui, t, state);
    center(ui, t, state, out);
    escalation_modal(&ctx, t, state, out);
    merge_modal(&ctx, t, state, out);
    if !state.escalations.is_empty() || state.rate_limit.is_some() || mission_is_live(state) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

pub use eframe::egui;
```

`mission_is_live`: `matches!(state.mission_state, Some(bridge_core::MissionState::Executing))`.

- [ ] **Step 3: Helper widgets (pills, cards, section labels)**

Add small helpers built on `Tokens` and `theme::tint`:

```rust
fn pill(ui: &mut egui::Ui, t: &Tokens, text: &str, color: Color32) {
    let bg = theme::tint(color, t.surface, 0.14);
    egui::Frame::new().fill(bg).corner_radius(999).inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| { ui.label(RichText::new(text).color(color).size(11.5)); });
}

fn card<R>(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new().fill(t.surface).stroke(egui::Stroke::new(1.0, t.hair_2))
        .corner_radius(13).inner_margin(egui::Margin::symmetric(20, 18))
        .show(ui, add).inner
}

fn section_label(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).color(t.text_3).size(11.0).strong());
}
```

Use `status_color(status, t)` mapping WorkstreamStatus -> semantic token (Merged/ReadyToMerge -> good; UnderTest -> info; Working/Rebasing/ConflictFix/InMergeQueue -> accent; Breached/Failed/Flagged -> crit; Pending -> text_3), and `decision_color(DecisionKind, t)` (Allow -> good, Deny -> crit, Escalate -> warn).

- [ ] **Step 4: Sidebar (mission header + list, no footer)**

`left_sidebar` = `egui::SidePanel::left("sidebar").exact_width(238.0).frame(Frame fill=sidebar)`, containing:
- Mission header: a horizontal row with "Bridge" (size 13, strong) and a right-aligned ellipsis-circle menu button (`egui::menu::menu_button` or a small circular `Button` showing `⋯`) whose items are "Wind down" (-> `BridgeCommand::WindDown`) and "Stop" (-> `BridgeCommand::Shutdown`).
- Mission title with a leading state dot: draw an 9px filled circle in the mission-state colour (`mission_state_color(state, t)`: Executing -> accent, Paused -> warn, Complete -> good, Failed -> crit, else text_3), then the mission objective text (`state.mission_detail` is the objective during Planning; otherwise show the mission title if tracked - use `mission_state_label` only in the tooltip). Use `ui.painter().circle_filled(...)` for the dot with `on_hover_text(mission_state_label(...))`.
- `section_label(ui, t, "Workstreams")` then a scroll area of rows. Each row: a selectable region; on selected, paint a `tint(accent, sidebar, 0.14)` rounded background; content = status dot (status_color) + name (short id or slug, size 13.5) + `status_label(status)` subtitle (size 11, text_2). Set `state.ui.selected` on click.

- [ ] **Step 5: Content (center)**

`center` = `egui::CentralPanel::default().frame(Frame fill=bg)`:
- If no `state.ui.selected` or no panel: show `section_label("Ship's Log")` + the global `state.logs` feed (mono rows coloured by level).
- Otherwise the selected `WorkstreamPanel`:
  - Title row: name (size 26, strong) + `pill(status_label, status_color)` + right-aligned **Open in VS Code** button (`egui::Button` filled `t.fill`, no stroke). On click, call `open_in_editor(state, selected)`. If `panel.worktree_path.is_none()`, disable the button (`add_enabled(false, ...)`).
  - Also, only while `Flagged`, an **Override** button -> `BridgeCommand::OverrideFlagged { workstream: selected }`.
  - Subtitle (mono, text_3): `station · branch` from the latest turn / spec; if unavailable, show the short id.
  - Battle report card via `card(...)`: newest report for this workstream, verdict coloured, stats row.
  - Turn history (recent, mono).
  - `section_label("Activity")` + the feed (tool_calls in info, output rows in text). Sticky-to-bottom scroll.

- [ ] **Step 6: `open_in_editor`**

```rust
fn open_in_editor(state: &AppState, ws: WorkstreamId) {
    let Some(path) = state.workstreams.get(&ws).and_then(|p| p.worktree_path.clone()) else {
        return;
    };
    let cmd = if state.ui.editor_command.is_empty() { "code".to_owned() } else { state.ui.editor_command.clone() };
    if let Err(e) = std::process::Command::new(&cmd).arg(&path).spawn() {
        tracing::warn!(editor = %cmd, path = %path.display(), error = %e, "failed to open editor");
    }
}
```

- [ ] **Step 7: Inspector (right)**

`right_inspector` = `egui::SidePanel::right("inspector").exact_width(292.0).frame(Frame fill=surface_2)`:
- `section_label("Merge queue")`; for each `state.merge_queue` entry: a numbered chip + branch + state text (Merged state in good).
- `section_label("Guardrails")` with a right-aligned count `"{total} checks - {clear} clear"` where `total = state.tactical_feed.len()` and `clear = count of Allow decisions`. Then render only entries where `decision != Allow` (denials + escalations), newest first: a badge (`decision_color`, sentence-case `Allow`/`Deny`/`Escalate`) + tool/command (mono) + reason caption. Then a "Show all activity" link (`ui.link`) that toggles `state.ui.show_all_guardrails` (add this `bool` to `UiInputs`); when true, also render the Allow rows.

- [ ] **Step 8: Composer (bottom)**

`bottom_composer` = `egui::TopBottomPanel::bottom("composer").frame(Frame fill=surface_2)`:
- A rounded `Frame` (fill surface, stroke accent when focused else hair, corner_radius 15) containing: a "Captain" tag (`pill`-like, accent tint), a `TextEdit::singleline(&mut state.ui.objective)` (frameless, hint "Give the Captain your next objective..."), and a trailing circular send button (accent fill, `↑`). Enter or the send button, when the objective is non-empty, pushes `BridgeCommand::StartMission { objective }` and clears it. Wind down / Stop are NOT here (they are in the sidebar menu).

- [ ] **Step 9: Banners and modals**

- `paused_banner`: only when `state.mission_state` is `Paused`. `warn`-tinted `TopBottomPanel::top`. For `RateLimited`, show `rate_limit_countdown_text`; for `BudgetExhausted`, show the reason and a "+20 turns" button -> `BridgeCommand::ExtendBudget(BudgetExtension { extra_total_turns: 20, ..Default::default() })`.
- `compat_banner`: unchanged behavior, restyled to a warn-tinted bar with "Proceed anyway" clearing `state.compat_warning`.
- `escalation_modal` and `merge_modal`: same logic as the current file, restyled with `t` colours (Approve good, Deny crit, Confirm accent). Keep using `state.remove_escalation` / `state.remove_merge_proposal` after emitting.

- [ ] **Step 10: Delete dead styling and confirm no red-alert refs**

Ensure the old `header`, `budget_row`, `rate_limit_row` (folded into paused banner), `red_alert_banner`, and LCARS consts are gone.

Run: `grep -n 'red_alert\|RedAlert\|AMBER\|LCARS' crates/bridge-app/src/ui.rs`
Expected: no matches.

- [ ] **Step 11: Compile and lint**

Run: `cargo check -p bridge-app`
Expected: PASS.
Run: `cargo clippy -p bridge-app --all-targets`
Expected: clean.
Run: `cargo test -p bridge-app`
Expected: PASS (reducer/theme tests still green).

- [ ] **Step 12: Manual launch checklist (both themes)**

Build and run against a scratch git repo (see the run skill / README). Set the OS to light, then dark, and confirm each: the window has no app toolbar; sidebar shows the mission header with a colored state dot (hover shows the state), the ⋯ menu offers Wind down / Stop, the workstream list selects with a soft accent fill; content shows the workstream title, status pill, Open in VS Code (spawns the editor on the worktree), battle report card (grouped, no shadow), and activity feed; the inspector shows the merge queue and exceptions-only Guardrails with the count and Show all; the composer submits on Enter and the send button; a paused mission shows the paused banner (with Extend on budget pause); no Red Alert affordance exists anywhere.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "feat: rewrite the GUI in the Apple-minimal design"
```

---

## Task 6: Final verification and docs

**Files:**
- Modify: `README.md` (drop any Red Alert mention; note the editor button / `ui.editor_command`).

- [ ] **Step 1: Full workspace gate**

Run: `cargo test --workspace`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets`
Expected: clean.
Run: `cargo fmt --all --check`
Expected: clean (run `cargo fmt --all` if not).

- [ ] **Step 2: Update the README**

In `README.md`, remove the Red Alert sentence from the configuration/quickstart sections, and add a line under configuration: "`[ui] editor_command` - the command the Open in VS Code button runs (default `code`)."

- [ ] **Step 3: End-to-end smoke (optional but recommended)**

Run a headless smoke mission (per README `--headless-smoke`) to confirm the removed Red Alert plumbing and the new event did not break mission flow.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: update README for GUI redesign (editor button, no Red Alert)"
```

---

## Self-Review Notes

Spec coverage checked section by section: token system (Task 4), layout with no toolbar / sidebar mission header / state dot / ellipsis menu / composer / inspector / content (Task 5), grouped-inset cards + soft selection + sentence-case badges (Tasks 4-5), exceptions-only Guardrails with count + show-all (Task 5), Open in VS Code + worktree-path plumbing + editor config (Tasks 2, 3, 5), Red Alert removed end to end incl. config and the MCP/decision-order rule (Task 1), paused/compat banners with budget extend (Task 5), light + dark both first-class (Task 4 resolver + Task 5 follows OS theme), no italics / no glyphs / no shadows (Global Constraints + Task 4/5), motion minimal with live-only repaint (Task 5 Step 2). Contracts stay except the three scoped changes. Type names are consistent across tasks (`theme::tokens/tint/visuals`, `ThemeMode`, `WorkstreamProvisioned { id, worktree_path }`, `UiConfig.editor_command`, `status_label`/`mission_state_label` Title case). egui 0.35 field-name risk is isolated to `theme.rs` with an explicit note.
