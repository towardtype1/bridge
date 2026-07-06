# Game Interaction Idiom (Sub-project C) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every remaining surface speaks the deck's language: pixel-chrome station console windows, an RPG dialogue box for the Captain conference and ship hails, pixel-strip banners; the Apple-minimal idiom retires completely.

**Architecture:** A single dark pixel theme replaces `theme.rs`'s light/dark pair (same `Tokens` struct and field names, new values, no `ThemeMode`, so consumers keep compiling). A new `pixel.rs` module holds the reusable chrome widgets (console frame, station header, chips, pixel buttons). A new `dialogue.rs` module holds the one dialogue component (pure `Typewriter` state + portrait texture + render fn) reused by the Captain conference, escalation hails, and merge hails. Fonts are committed TTF assets embedded via `FontDefinitions` at startup.

**Tech Stack:** Rust 2024, eframe/egui 0.35 (egui re-exported via `pub use eframe::egui;` in ui.rs), Press Start 2P + VT323 (both OFL, committed under `assets/fonts/`). No new crate dependencies.

**Spec:** `docs/superpowers/specs/2026-07-06-game-idiom-design.md`. Visual reference: https://claude.ai/code/artifact/e0270ab6-290a-464c-93ac-76a8b9163ba8

## Global Constraints

- Single dark theme: `Tokens` keeps its struct name and field names; values become: bg `#060811`, surface `#0e1322`, surface_2 `#121933`, sidebar `#121933` (kept for the selection fill it still feeds), text `#c9d4e8`, text_2 `#7c89a6`, text_3 `#55628a`, hair `#232c47`, hair_2 `#1a2138`, accent `#ffb648`, fill `#121933`, fill_2 `#1a2138`, good `#3fd68c`, warn `#ffb648`, crit `#ff5a4e`, info `#59c8ff`. `ThemeMode` and the light palette are DELETED; `tokens()` and `visuals()` take no arguments.
- Where a color is shared with the deck (amber/red/green/cyan), the theme re-exports `crate::deck::sprites::{AMBER, RED, GREEN, CYAN}` (they are `pub(crate)` Rgb consts) rather than redeclaring hex values.
- Corners square everywhere (`CornerRadius::ZERO`); no shadows.
- Font families registered as `"pixel"` (Press Start 2P) and `"crt"` (VT323); the egui default proportional family is NOT replaced.
- Typewriter: ~2 chars per 16 ms, time-based from a start instant; click-to-complete; `ui.reduce_motion` renders complete immediately; blinking block cursor while revealing.
- Diff markers in the briefing card: `+` (good) added, `~` (warn) revised, `>` (text_3) unchanged, red `x {slug} (cancelled)` rows for removals.
- No new backend events/commands; presentation only. `bridge-app` + `assets/` only.
- All work on branch `game-idiom` off main. Every task ends green: `cargo test -p bridge-app`, `cargo build --workspace 2>&1` zero warnings, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
- Conventional commits, NO co-author lines. Tests in inline `#[cfg(test)]` modules.

---

### Task 1: Font assets + the pixel theme

**Files:**
- Create: `assets/fonts/PressStart2P-Regular.ttf`, `assets/fonts/PressStart2P-OFL.txt`, `assets/fonts/VT323-Regular.ttf`, `assets/fonts/VT323-OFL.txt`
- Rewrite: `crates/bridge-app/src/theme.rs`
- Modify: `crates/bridge-app/src/main.rs` (install fonts once at app creation; drop ThemeMode plumbing)
- Modify: `crates/bridge-app/src/ui.rs:43-66` (`draw` no longer resolves a theme mode)

**Interfaces:**
- Produces: `theme::tokens() -> Tokens` (no args), `theme::visuals() -> egui::Visuals` (no args), `theme::install_fonts(ctx: &egui::Context)`, `theme::pixel(size: f32) -> egui::FontId`, `theme::crt(size: f32) -> egui::FontId`. `Tokens` fields unchanged in name/type.

- [ ] **Step 1: Fetch and commit the font assets**

```bash
mkdir -p assets/fonts && cd assets/fonts
curl -sL -o PressStart2P-Regular.ttf "https://github.com/google/fonts/raw/main/ofl/pressstart2p/PressStart2P-Regular.ttf"
curl -sL -o PressStart2P-OFL.txt "https://raw.githubusercontent.com/google/fonts/main/ofl/pressstart2p/OFL.txt"
curl -sL -o VT323-Regular.ttf "https://github.com/google/fonts/raw/main/ofl/vt323/VT323-Regular.ttf"
curl -sL -o VT323-OFL.txt "https://raw.githubusercontent.com/google/fonts/main/ofl/vt323/OFL.txt"
file *.ttf   # both must report "TrueType Font data" - HTML error pages are the failure mode
```

If GitHub is unreachable, STOP and report BLOCKED (the fonts are required, embedded at compile time).

- [ ] **Step 2: Write the failing tests** (new tests module in `theme.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_the_pixel_palette() {
        let t = tokens();
        assert_eq!(t.bg, egui::Color32::from_rgb(0x06, 0x08, 0x11));
        assert_eq!(t.accent, egui::Color32::from_rgb(0xff, 0xb6, 0x48));
        assert_eq!(t.info, egui::Color32::from_rgb(0x59, 0xc8, 0xff));
        assert_eq!(t.crit, egui::Color32::from_rgb(0xff, 0x5a, 0x4e));
    }

    #[test]
    fn visuals_are_square_and_shadowless() {
        let v = visuals();
        assert_eq!(v.window_corner_radius, egui::CornerRadius::ZERO);
        assert_eq!(v.window_shadow, egui::epaint::Shadow::NONE);
        assert!(v.dark_mode);
    }

    #[test]
    fn fonts_register_pixel_and_crt_families() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let families = ctx.fonts(|f| f.families());
        assert!(families.contains(&egui::FontFamily::Name("pixel".into())));
        assert!(families.contains(&egui::FontFamily::Name("crt".into())));
    }
}
```

(egui 0.35 note: if `fonts.families()` does not exist, read the registered families via `ctx.fonts(|f| f.lock().fonts.definitions().families.keys()...)` or simply assert `egui::FontId { family: FontFamily::Name("pixel".into()), size: 10.0 }` lays out text without panicking inside `ctx.run(Default::default(), |ctx| { ... })`. Choose whichever compiles; the assertion's intent is "both families are registered".)

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p bridge-app theme` - Expected: FAIL to compile (`tokens()` takes no such signature yet).

- [ ] **Step 4: Rewrite `theme.rs`**

```rust
//! The single dark pixel theme. One committed visual world: square corners,
//! no shadows, phosphor accents shared with the deck's sprite constants.

use crate::deck::sprites::{AMBER, CYAN, GREEN, RED};
use crate::ui::egui;
use egui::Color32;

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

fn deck(c: crate::deck::sprites::Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

pub fn tokens() -> Tokens {
    Tokens {
        bg: rgb(0x06, 0x08, 0x11),
        surface: rgb(0x0e, 0x13, 0x22),
        surface_2: rgb(0x12, 0x19, 0x33),
        sidebar: rgb(0x12, 0x19, 0x33),
        text: rgb(0xc9, 0xd4, 0xe8),
        text_2: rgb(0x7c, 0x89, 0xa6),
        text_3: rgb(0x55, 0x62, 0x8a),
        hair: rgb(0x23, 0x2c, 0x47),
        hair_2: rgb(0x1a, 0x21, 0x38),
        accent: deck(AMBER),
        fill: rgb(0x12, 0x19, 0x33),
        fill_2: rgb(0x1a, 0x21, 0x38),
        good: deck(GREEN),
        warn: deck(AMBER),
        crit: deck(RED),
        info: deck(CYAN),
    }
}

/// egui-wide visuals for the pixel world: dark, square, shadowless.
pub fn visuals() -> egui::Visuals {
    let t = tokens();
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(t.text);
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.window_stroke = egui::Stroke::new(2.0, t.info);
    v.window_corner_radius = egui::CornerRadius::ZERO;
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.active.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.open.corner_radius = egui::CornerRadius::ZERO;
    v.selection.bg_fill = t.sidebar;
    v
}

/// Register the committed font assets under the "pixel" and "crt" families.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "press-start-2p".into(),
        egui::FontData::from_static(include_bytes!(
            "../../../assets/fonts/PressStart2P-Regular.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "vt323".into(),
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/VT323-Regular.ttf"))
            .into(),
    );
    fonts.families.insert(
        egui::FontFamily::Name("pixel".into()),
        vec!["press-start-2p".into()],
    );
    fonts
        .families
        .insert(egui::FontFamily::Name("crt".into()), vec!["vt323".into()]);
    ctx.set_fonts(fonts);
}

pub fn pixel(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("pixel".into()))
}

pub fn crt(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("crt".into()))
}

/// Blend `base` at `pct` (0..=1) over the opaque `over` ground.
pub fn tint(base: Color32, over: Color32, pct: f32) -> Color32 {
    let l = |a: u8, b: u8| (a as f32 * pct + b as f32 * (1.0 - pct)) as u8;
    Color32::from_rgb(
        l(base.r(), over.r()),
        l(base.g(), over.g()),
        l(base.b(), over.b()),
    )
}
```

(Keep `tint` - existing callers use it. egui 0.35 escape hatch: if `FontData::from_static(...).into()` does not satisfy the map's value type, check whether `font_data` wants `FontData` directly or `Arc<FontData>` in the vendored egui source and adapt; likewise `FontDefinitions::families` key type. Delete any now-unused theme tests from the old file.)

- [ ] **Step 5: Chase the compiler**

`ui.rs::draw` becomes:

```rust
pub fn draw(
    ctx: &egui::Context,
    state: &mut AppState,
    deck: &mut crate::deck::render::DeckCanvas,
    ui_cfg: &bridge_core::UiConfig,
    out_commands: &mut Vec<BridgeCommand>,
) {
    ctx.set_visuals(theme::visuals());
    let t = theme::tokens();
    // ... unchanged below
}
```

`main.rs`: in `run_gui`'s creation closure, call `theme::install_fonts(&cc.egui_ctx);` before constructing `BridgeApp`. Remove every `ThemeMode` reference workspace-wide (grep `ThemeMode` - the compiler drives it). If `deck::sprites`' color consts are not yet `pub(crate)` visible from theme.rs, adjust their visibility (they were made `pub(crate)` in the deck fix wave).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p bridge-app` - Expected: PASS (all, including the 3 new). Then the standard gates (build zero warnings, fmt, clippy).

- [ ] **Step 7: Commit**

```bash
git add assets crates/bridge-app
git commit -m "feat(theme): single dark pixel theme with embedded Press Start 2P and VT323"
```

---

### Task 2: `pixel.rs` - console chrome widgets

**Files:**
- Create: `crates/bridge-app/src/pixel.rs`
- Modify: `crates/bridge-app/src/main.rs` (add `mod pixel;`)

**Interfaces:**
- Produces (all `pub(crate)`, all taking `t: &Tokens`):
  - `pub enum ChipKind { Allow, Deny, Escalate, Info, Good, Warn, Crit }` and `pub fn chip_color(kind: ChipKind, t: &Tokens) -> Color32` (pure, tested).
  - `pub fn chip(ui, t, label: &str, kind: ChipKind)` - pixel font 8.0, uppercase label as given, 1 px border in the kind color, fill = `theme::tint(color, t.surface, 0.12)`, 2 px inner margin, square.
  - `pub fn station_header(ui, t, title: &str)` - pixel font 10.0, `t.info`, uppercase, 4.0 space after.
  - `pub fn pixel_button(ui, t, label: &str, fill: Color32) -> egui::Response` - pixel font 9.0, dark text (`t.bg`) on `fill`, square, min size y 26.0, and a 2 px darker bottom edge painted under the rect (`theme::tint(Color32::BLACK, fill, 0.35)`).
  - `pub fn console_window<'a>(ctx, t, title: &str, id: &str, open: &'a mut bool) -> egui::Window<'a>`-style helper is NOT feasible with egui's builder lifetimes; instead produce `pub fn console_frame(t: &Tokens) -> egui::Frame` - fill `#080c18`, `stroke (2.0, t.info)`, inner margin 14, square - which each window passes to `egui::Window::frame`, plus `pub fn double_outline(ui: &mut egui::Ui, t: &Tokens)` painting the 1 px outer hairline 3 px outside `ui.max_rect()` via `ui.painter().rect_stroke(...)`.

- [ ] **Step 1: Write the failing test** (chip mapping is the pure core)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    #[test]
    fn chip_colors_map_semantically() {
        let t = theme::tokens();
        assert_eq!(chip_color(ChipKind::Allow, &t), t.good);
        assert_eq!(chip_color(ChipKind::Deny, &t), t.crit);
        assert_eq!(chip_color(ChipKind::Escalate, &t), t.warn);
        assert_eq!(chip_color(ChipKind::Info, &t), t.info);
        assert_eq!(chip_color(ChipKind::Good, &t), t.good);
        assert_eq!(chip_color(ChipKind::Warn, &t), t.warn);
        assert_eq!(chip_color(ChipKind::Crit, &t), t.crit);
    }
}
```

- [ ] **Step 2: Run to verify failure** - `cargo test -p bridge-app pixel` - FAIL to compile.

- [ ] **Step 3: Implement `pixel.rs`**

```rust
//! Reusable pixel-chrome widgets: console frames, station headers, chips,
//! and pixel buttons. Pure presentation over theme::Tokens.

use crate::theme::{self, Tokens};
use crate::ui::egui;
use egui::{Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    Allow,
    Deny,
    Escalate,
    Info,
    Good,
    Warn,
    Crit,
}

pub fn chip_color(kind: ChipKind, t: &Tokens) -> Color32 {
    match kind {
        ChipKind::Allow | ChipKind::Good => t.good,
        ChipKind::Deny | ChipKind::Crit => t.crit,
        ChipKind::Escalate | ChipKind::Warn => t.warn,
        ChipKind::Info => t.info,
    }
}

pub fn chip(ui: &mut egui::Ui, t: &Tokens, label: &str, kind: ChipKind) {
    let color = chip_color(kind, t);
    egui::Frame::new()
        .fill(theme::tint(color, t.surface, 0.12))
        .stroke(egui::Stroke::new(1.0, color))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).font(theme::pixel(8.0)).color(color));
        });
}

pub fn station_header(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.label(
        RichText::new(title.to_uppercase())
            .font(theme::pixel(10.0))
            .color(t.info),
    );
    ui.add_space(4.0);
}

pub fn pixel_button(ui: &mut egui::Ui, t: &Tokens, label: &str, fill: Color32) -> egui::Response {
    let resp = ui.add(
        egui::Button::new(RichText::new(label).font(theme::pixel(9.0)).color(t.bg))
            .fill(fill)
            .corner_radius(0)
            .min_size(egui::vec2(0.0, 26.0)),
    );
    let r = resp.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.max.y), egui::pos2(r.max.x, r.max.y + 2.0)),
        0.0,
        theme::tint(Color32::BLACK, fill, 0.35),
    );
    resp
}

pub fn console_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x08, 0x0c, 0x18))
        .stroke(egui::Stroke::new(2.0, t.info))
        .inner_margin(14)
}

/// The mockup's outer hairline, 3 px outside the frame.
pub fn double_outline(ui: &mut egui::Ui, t: &Tokens) {
    let r = ui.max_rect().expand(5.0);
    ui.painter()
        .rect_stroke(r, 0.0, egui::Stroke::new(1.0, t.hair), egui::StrokeKind::Outside);
}
```

(egui 0.35 note: `rect_stroke`'s `StrokeKind` argument exists in 0.35; if the signature differs, use the form the vendored source requires. `corner_radius(0)` on Button exists per the repo's prior usage.)

- [ ] **Step 4: Run tests + gates.** `cargo test -p bridge-app` PASS; build/fmt/clippy clean (the not-yet-consumed pub(crate) fns may warn as dead code - if so, consume nothing yet but add `#![allow(dead_code)] // consumed by Tasks 3-6; remove in Task 7` at the top of pixel.rs with that exact comment).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-app/src/pixel.rs crates/bridge-app/src/main.rs
git commit -m "feat(app): pixel chrome widgets - console frames, chips, buttons"
```

---

### Task 3: Station console chrome on all five windows

**Files:**
- Modify: `crates/bridge-app/src/ui.rs` (`workstream_window` ~552, `tactical_window` ~691, `kobayashi_window` ~706, `computer_window`, `mission_status_window`, plus `guardrail_row` ~496 and the kobayashi/status row renderers where chips replace pills)

**Interfaces:**
- Consumes: `pixel::{console_frame, station_header, chip, ChipKind, pixel_button, double_outline}`.
- Produces: no new API; the five windows restyled. Window content (data shown, commands pushed) is UNCHANGED.

- [ ] **Step 1: Restyle each window.** The pattern, applied to all five (shown here for tactical; repeat mechanically for the others with their titles):

```rust
fn tactical_window(ctx: &egui::Context, t: &Tokens, state: &mut AppState) {
    let mut open = state.ui.open_tactical;
    if !open {
        return;
    }
    egui::Window::new("tactical_console")
        .title_bar(false)
        .frame(crate::pixel::console_frame(t))
        .default_size(egui::vec2(520.0, 420.0))
        .open(&mut open)
        .show(ctx, |ui| {
            crate::pixel::double_outline(ui, t);
            ui.horizontal(|ui| {
                crate::pixel::station_header(ui, t, "Tactical - guardrail adjudications");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::pixel::pixel_button(ui, t, "CLOSE", t.fill_2).clicked() {
                        state.ui.open_tactical = false;
                    }
                });
            });
            ui.add_space(8.0);
            guardrails_group(ui, t, state);
        });
    if !open {
        state.ui.open_tactical = false;
    }
}
```

Notes that apply to all five:
- `title_bar(false)` + the in-frame `station_header` + a CLOSE pixel button replaces the native title bar (the mockup has no OS-style title bars). Since `.open(&mut open)`'s close button lives in the title bar, closing is now the CLOSE button (and the existing Esc handling if present); keep the `open` write-back pattern for state consistency.
- Titles: workstream = `format!("Helm console - {}", short_id(&selected))`; kobayashi = "Kobayashi Maru - battle report"; computer = "Ship's computer - log"; mission status = "Mission status".
- The workstream window's close must still clear `state.ui.selected` (existing behavior).
- Inside content: replace `pill(...)` calls with `chip(...)`: workstream status pill -> `chip(ui, t, &status_label(status), kind)` where kind maps Working->Warn, Merged/ReadyToMerge->Good, Breached/Failed/Flagged->Crit, UnderTest->Info, else Info; guardrail rows' decision label -> `chip` with Allow/Deny/Escalate kinds via the existing `DecisionKind`; kobayashi severity text -> `chip` with Crit/Warn/Info by severity. Keep `pill` itself for now (banners still use it until Task 6; Task 7 sweeps).
- Buttons inside these windows ("Open in VS Code", "Override", Wind down, Stop) become `pixel_button` (Override and Stop use `t.crit`; others `t.fill_2` with... `pixel_button` draws dark text - for fill_2 the label would be unreadable dark-on-dark: give `pixel_button` callers a readable pairing by using `t.info` fill for neutral actions instead of fill_2 in these windows; CLOSE uses `t.info` too. Adjust the Task 2 helper if needed so neutral buttons are `t.info`-filled with `t.bg` text, destructive `t.crit`, affirmative `t.good`).
- Body text inside windows: where labels currently use `.size(13.0)`-ish RichText, leave sizes but set `.font(theme::crt(17.0))` for log/terminal-like content (ships_log lines, activity feed, tool calls) - CRT face for readouts per the spec's split. Headers/labels within content (section_label calls) stay as-is this task.

- [ ] **Step 2: Build + visual sanity.** `cargo build -p bridge-app` then `cargo run --release` briefly (any repo): open each window via deck clicks; confirm chrome, chips, CLOSE works, no layout explosions. This task is visual - eyeball it yourself before committing (screenshot if display available).

- [ ] **Step 3: Gates + commit**

```bash
git add crates/bridge-app/src/ui.rs
git commit -m "feat(app): station console chrome for all detail windows"
```

---

### Task 4: `dialogue.rs` - typewriter, portraits, the dialogue component

**Files:**
- Create: `crates/bridge-app/src/dialogue.rs`
- Modify: `crates/bridge-app/src/deck/sprites.rs` (add `PORTRAIT` 28x28 map + portrait palette per speaker)
- Modify: `crates/bridge-app/src/main.rs` (add `mod dialogue;`)

**Interfaces:**
- Produces:
  - `pub struct Typewriter { text: String, started: f64, done: bool }` with `new(text, now)`, `visible(&self, now: f64, reduce_motion: bool) -> &str` (chars revealed = `((now - started) * 125.0) as usize`, i.e. 2 chars per 16 ms; all when done/reduce_motion), `complete(&mut self)`, `is_done(&self, now, reduce_motion) -> bool`.
  - `pub enum Speaker { Captain, Tactical, Helm }` with `name(&self) -> &'static str` ("CAPTAIN"/"TACTICAL - PRIORITY HAIL"/"HELM") and `accent(&self, t: &Tokens) -> Color32` (accent/crit/warn).
  - `pub struct DialogueChoice { pub label: &'static str, pub fill: fn(&Tokens) -> Color32 }` - avoided; instead choices are rendered by the CALLER via a closure: `pub fn dialogue_box(ctx, t, id: &str, speaker: Speaker, tw: &Typewriter, now: f64, reduce_motion: bool, add_contents: impl FnOnce(&mut egui::Ui)) -> DialogueResponse` where `DialogueResponse { pub closed: bool, pub text_clicked: bool }`. The component renders chrome, portrait, speaker name, typewriter body (+cursor), then hands the ui to `add_contents` for cards/choices/inputs; the caller wires buttons itself with `pixel_button`.
  - `pub fn portrait_texture(ctx: &egui::Context, speaker: Speaker) -> egui::TextureHandle` cached per speaker in a `std::sync::OnceLock`-free way: accept a `&mut Option<egui::TextureHandle>` slot from the caller instead (UI state owns caching; no globals).
  - In `sprites.rs`: `pub const PORTRAIT: SpriteMap` (28 rows x 28 cols, chars H,K,W,E,N,M,U,D,'.') and `pub fn portrait_color(map_char: u8, speaker: PortraitSpeaker) -> Option<Rgb>` with `pub enum PortraitSpeaker { Command, Tactical, Helm }` - Command: hair #5a4632, uniform #8c2f39, pips #d9a441; Tactical: hair #1a1a22, uniform #c9903a; Helm: hair #8c6a3a, uniform #c9903a. Shared: skin #e0b48c, eyes #2a2a3a, whites #ffffff, nose #c99a74, mouth #b07850, dark accent #5e1f26 (Command) / #8a5f1e (gold speakers).

- [ ] **Step 1: Add `PORTRAIT` to sprites.rs** - copy VERBATIM from the mockup (this is the authoritative map; chars: H hair, K skin, W eye-white, E eye, N nose, M mouth, U uniform, D collar accent):

```rust
/// 28x28 front bust for the dialogue box; palette-swapped per speaker.
pub const PORTRAIT: SpriteMap = &[
    "............................",
    ".........HHHHHHHHHH.........",
    ".......HHHHHHHHHHHHHH.......",
    "......HHHHHHHHHHHHHHHH......",
    ".....HHHHHHHHHHHHHHHHHH.....",
    ".....HHHHHHHHHHHHHHHHHH.....",
    ".....HHKKKKKKKKKKKKKKHH.....",
    ".....HKKKKKKKKKKKKKKKKH.....",
    ".....HKKKKKKKKKKKKKKKKH.....",
    ".....HKKWWKKKKKKKKWWKKH.....",
    ".....HKKEWKKKKKKKKEWKKH.....",
    ".....HKKKKKKKKKKKKKKKKH.....",
    ".....HKKKKKKKNNKKKKKKKH.....",
    ".....HKKKKKKKNNKKKKKKKH.....",
    "......KKKKKKKKKKKKKKKK......",
    "......KKKMMMMMMMMMMKKK......",
    ".......KKKKKKKKKKKKKK.......",
    "........KKKKKKKKKKKK........",
    ".........KKKKKKKKKK.........",
    "......UUUUUKKKKKKUUUUU......",
    "....UUUUUUUUKKKKUUUUUUUU....",
    "...UUUUUUUUUUUUUUUUUUUUU....",
    "...UUUUUUUUUUUUUUUUUUUUUU...",
    "..UUUUUUUUUUUUUUUUUUUUUUUU..",
    "..UUUDDUUUUUUUUUUUUUUUUUUU..",
    "..UUUDDUUUUUUUUUUUUUUUUUUU..",
    "..UUUDDUUUUUUUUUUUUUUUUUUU..",
    "............................",
];
```

Add `PortraitSpeaker` + `portrait_color` mapping the palettes listed in Interfaces. Extend the sprites tests: PORTRAIT is 28x28 rectangular; every non-'.' char resolves for all three speakers.

- [ ] **Step 2: Write the failing typewriter tests** (in `dialogue.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typewriter_reveals_at_125_chars_per_second() {
        let tw = Typewriter::new("HELLO CREW".into(), 10.0);
        assert_eq!(tw.visible(10.0, false), "");
        assert_eq!(tw.visible(10.024, false), "HEL"); // 0.024s * 125 = 3
        assert_eq!(tw.visible(11.0, false), "HELLO CREW");
        assert!(tw.is_done(11.0, false));
        assert!(!tw.is_done(10.01, false));
    }

    #[test]
    fn typewriter_completes_on_click_and_reduce_motion() {
        let mut tw = Typewriter::new("HELLO".into(), 0.0);
        assert_eq!(tw.visible(0.0, true), "HELLO");
        assert!(tw.is_done(0.0, true));
        tw.complete();
        assert_eq!(tw.visible(0.001, false), "HELLO");
    }

    #[test]
    fn typewriter_is_char_safe_on_multibyte() {
        let tw = Typewriter::new("réplique".into(), 0.0);
        // must never split a UTF-8 char - walk chars, not bytes
        let _ = tw.visible(0.008, false); // 1 char
    }
}
```

- [ ] **Step 3: Run to verify failure**, then implement the pure core:

```rust
//! The RPG dialogue box: pure typewriter state plus the egui component
//! reused by the Captain conference, Tactical escalation hails, and Helm
//! merge hails.

use crate::deck::scene::{NATIVE_H, NATIVE_W};
use crate::deck::sprites::{portrait_color, PortraitSpeaker, PORTRAIT};
use crate::pixel;
use crate::theme::{self, Tokens};
use crate::ui::egui;
use egui::{Color32, RichText};

const CHARS_PER_SEC: f64 = 125.0;

#[derive(Debug, Clone)]
pub struct Typewriter {
    text: String,
    started: f64,
    done: bool,
}

impl Typewriter {
    pub fn new(text: String, now: f64) -> Self {
        Self { text, started: now, done: false }
    }

    pub fn visible(&self, now: f64, reduce_motion: bool) -> &str {
        if self.done || reduce_motion {
            return &self.text;
        }
        let n = ((now - self.started).max(0.0) * CHARS_PER_SEC) as usize;
        match self.text.char_indices().nth(n) {
            Some((byte, _)) => &self.text[..byte],
            None => &self.text,
        }
    }

    pub fn is_done(&self, now: f64, reduce_motion: bool) -> bool {
        self.done || reduce_motion || self.visible(now, false).len() == self.text.len()
    }

    pub fn complete(&mut self) {
        self.done = true;
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}
```

Then `Speaker`, `portrait_texture(ctx, speaker, slot: &mut Option<egui::TextureHandle>)` (paint PORTRAIT into a 28x28 `ColorImage` via `portrait_color`, `TextureOptions::NEAREST`, cache in the slot), and `dialogue_box` rendering: bottom-anchored `egui::Area` (`egui::Align2::CENTER_BOTTOM`, offset -24.0) containing a `Frame` (fill `Color32::from_rgba_unmultiplied(6, 9, 20, 245)`, stroke `(3.0, #e8ecf5)`, inner margin 14, max width = 94 percent of available), horizontal: portrait image at 84x84 (3x scale, `egui::Image` from the texture) in a 2 px `t.hair`-bordered box, then vertical: speaker name (`pixel(10.0)`, speaker accent), typewriter body (`crt(19.0)`, `t.text`) with a trailing block cursor while not done (`▉`-free: paint a `6x14` rect blinking on `(now * 2.0) as i64 % 2 == 0` - reduce_motion shows no cursor), then `add_contents(ui)`. Clicking the body text region completes: return `text_clicked` when a click lands in the body's response; a CLOSE button (pixel_button, top-right, `t.fill_2`... use `t.info`) sets `closed`. While any typewriter is unfinished, request repaint every 16 ms (`ctx.request_repaint_after`).

- [ ] **Step 4: Tests + gates.** `cargo test -p bridge-app` PASS (typewriter + sprites portrait tests). Dead-code allow with the Task-7 removal comment if needed.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-app/src/dialogue.rs crates/bridge-app/src/deck/sprites.rs crates/bridge-app/src/main.rs
git commit -m "feat(app): dialogue component - typewriter, portraits, RPG chrome"
```

---

### Task 5: The Captain conference dialogue (replaces captain window + composer)

**Files:**
- Modify: `crates/bridge-app/src/ui.rs` (delete `bottom_composer` ~321 and `captain_window`; new `captain_dialogue`; `draw_in` drops the composer panel; a floating HAIL button)
- Modify: `crates/bridge-app/src/state.rs` (`UiInputs` gains `captain_tw: Option<(usize, dialogue::Typewriter)>` - hmm, `Typewriter` in state: `UiInputs` derives Debug/Default; Typewriter derives Debug+Clone; wrap as `pub captain_tw: Option<CaptainTw>` where `pub struct CaptainTw { pub feed_len: usize, pub tw: crate::dialogue::Typewriter }` - lives in state.rs, derives Debug. Also `pub captain_portrait: Option<egui::TextureHandle>` (skip Debug via manual impl? egui TextureHandle implements Debug? If not, keep the portrait cache OUT of AppState: store it in `DeckCanvas`-style app-shell state - add `pub struct DialogueTextures { pub captain: Option<egui::TextureHandle>, pub tactical: Option<egui::TextureHandle>, pub helm: Option<egui::TextureHandle> }` owned by `BridgeApp` in main.rs and passed into `ui::draw` alongside `deck`.)

**Interfaces:**
- Consumes: `dialogue::{Typewriter, Speaker, dialogue_box, portrait_texture}`, `pixel::pixel_button`, existing `captain_feed`/`latest_proposal`/`SayToCaptain`/`ApproveProposal`.
- Produces: `ui::draw(ctx, state, deck, textures: &mut DialogueTextures, ui_cfg, out)` (signature change; main.rs is the only caller); `fn captain_dialogue(ctx, t, state, textures, ui_cfg, out)`.

- [ ] **Step 1: State + typewriter sync test** (state.rs)

```rust
#[test]
fn captain_typewriter_restarts_on_new_message() {
    let mut s = AppState::default();
    let mission = MissionId::new();
    s.apply(BridgeEvent::CaptainSays { mission, text: "First.".into() });
    let tw1 = s.ui.sync_captain_tw(1.0, /* reduce_motion */ false);
    assert_eq!(tw1.tw.text(), "First.");
    // same feed length -> same typewriter instance (started stays 1.0)
    let started = { s.ui.sync_captain_tw(2.0, false); s.ui.captain_tw.as_ref().unwrap().tw.visible(1.001, false).len() };
    assert!(started <= "First.".len());
    s.apply(BridgeEvent::CaptainSays { mission, text: "Second.".into() });
    let tw2 = s.ui.sync_captain_tw(3.0, false);
    assert_eq!(tw2.tw.text(), "Second.");
}
```

Implement `UiInputs::sync_captain_tw(&mut self, now: f64, ...) -> &CaptainTw`: finds the last CAPTAIN-speaker entry in `captain_feed`... `captain_feed` lives on `AppState`, not `UiInputs` - make it `AppState::sync_captain_tw(&mut self, now: f64) -> Option<&CaptainTw>`: if the feed's last Captain message index differs from `captain_tw.feed_len`, create a fresh `Typewriter::new(text, now)`; return the current one. (Adapt the test to the chosen home; the intent is: new Captain message restarts the typewriter, unchanged feed keeps it.)

- [ ] **Step 2: Implement `captain_dialogue`** (replaces `captain_window`; called from `center()` where captain_window was):

- Open when `state.ui.open_captain`.
- Body: `state.sync_captain_tw(now)`'s typewriter (or a static "Standing by, sir." Typewriter-complete text when the feed is empty); above it, a compact scrollback (`ScrollArea`, `crt(16.0)`, `t.text_2` for You / `t.accent` names) of earlier feed entries, max_height 120.0.
- Card: when `state.latest_proposal` is Some, render inside `add_contents`: amber-bordered `Frame` (stroke `(2.0, theme::tint(t.accent, t.surface, 0.6))`, fill `tint(t.accent, t.surface, 0.05)`), header `MISSION BRIEFING - REVISION {N}` (pixel 9.0, accent) or `PLAN AMENDMENT - DIFF vs CURRENT` when `diff.is_some()`, rows in `crt(17.0)` with the markers from Global Constraints, then choices row: `pixel_button(ui, t, "MAKE IT SO", t.good)` -> `out.push(BridgeCommand::ApproveProposal { revision })`.
- Input row (always, at the bottom): TextEdit (crt 18.0, hint "Say something to the Captain...") reusing `state.ui.objective` as the buffer + `pixel_button(ui, t, "SEND", t.info)`; Enter or SEND pushes `BridgeCommand::SayToCaptain { text }` and clears the buffer (exact behavior the composer had).
- `text_clicked` completes the typewriter (`captain_tw.tw.complete()`).
- `closed` sets `open_captain = false`.

- [ ] **Step 3: Delete `bottom_composer` and its `draw_in` call.** Add the floating hail button: in `center()` after the windows, when `!state.ui.open_captain` (and no hail dialogue from Task 6 is open - use a small `fn any_dialogue_open(state) -> bool` that Task 6 extends):

```rust
egui::Area::new(egui::Id::new("hail_button"))
    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
    .show(&ctx, |ui| {
        if pixel_button(ui, t, "HAIL THE CAPTAIN", t.accent).clicked() {
            state.ui.open_captain = true;
        }
    });
```

- [ ] **Step 4: Gates + manual smoke.** All four gates; then `cargo run --release` in a scratch repo: HAIL button opens the dialogue, typing sends (watch UserSaid echo appear in scrollback), typewriter animates and completes on click. Screenshot if display available.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-app/src
git commit -m "feat(app): Captain conference as RPG dialogue; composer retired"
```

---

### Task 6: Escalation and merge hails

**Files:**
- Modify: `crates/bridge-app/src/ui.rs` (delete `escalation_modal` and `merge_modal`; new `hail_dialogue` called from `draw_in` in their place)
- Modify: `crates/bridge-app/src/state.rs` (typewriter cache for the active hail: `pub hail_tw: Option<HailTw>` where `HailTw { pub key: String, pub tw: dialogue::Typewriter }` - key is the escalation id / workstream id string so a new hail restarts the reveal)

**Interfaces:**
- Consumes: `dialogue_box`, `Speaker::{Tactical, Helm}`, existing `state.escalations` / `state.pending_merges` / `state.ui.deny_reasons`, commands `ResolveEscalation`/`ConfirmMerge`.
- Produces: `fn hail_dialogue(ctx, t, state, textures, ui_cfg, out)`; `any_dialogue_open` covers hails.

- [ ] **Step 1: Queue-order test** (pure fn; state.rs or ui.rs tests)

```rust
#[test]
fn hail_queue_prefers_escalations_then_merges_in_arrival_order() {
    let mut s = AppState::default();
    let m1 = proposal(); // existing test helper for MergeProposal
    s.apply(BridgeEvent::MergeConfirmationRequested(m1.clone()));
    let t1 = ticket(); // existing test helper for EscalationTicket
    s.apply(BridgeEvent::EscalationRequested(t1.clone()));
    match s.active_hail() {
        Some(Hail::Escalation(e)) => assert_eq!(e.id, t1.id),
        other => panic!("expected escalation first, got {other:?}"),
    }
    s.apply(BridgeEvent::EscalationResolved { id: t1.id, decision: UserDecision::Approve });
    assert!(matches!(s.active_hail(), Some(Hail::Merge(_))));
}
```

Implement `pub enum Hail<'a> { Escalation(&'a EscalationTicket), Merge(&'a MergeProposal) }` and `AppState::active_hail(&self) -> Option<Hail>` = first escalation, else first pending merge (this mirrors the current modal precedence: check the existing `escalation_modal`/`merge_modal` call order in draw_in and match it exactly - if merges currently render first, keep THAT order and flip the test).

- [ ] **Step 2: Implement `hail_dialogue`.** One dialogue at a time from `active_hail()`:

- Escalation: `Speaker::Tactical`, body text = `format!("{} requests: {}. {}", ticket-station-or-workstream, ticket summary fields, "Standing orders do not cover this. No answer means DENY.")` - build from the same `EscalationTicket` fields `escalation_modal` renders today (copy its field usage verbatim, reworded into one narration string; keep the countdown seconds, appended like "T-minus {}s."). Choices: `APPROVE ONCE` (good) -> `ResolveEscalation { id, decision: UserDecision::Approve }`; `DENY` (crit) -> `UserDecision::Deny { reason }` from the existing `deny_reasons` buffer, rendered as a small `crt(17.0)` TextEdit labeled `REASON` above the choices; plus whatever third variant the current modal offers (check `UserDecision` - if an always-allow/pattern variant exists in the modal today, keep it; if not, do NOT invent one).
- Merge: `Speaker::Helm`, body = `format!("Workstream {} requests permission to dock. Branch {} rebased clean.", short_id(&p.workstream), p.branch)` plus the fields `merge_modal` shows today (diff stat if present). Choices: `ENGAGE` (good) -> `ConfirmMerge { workstream, approved: true }`; `HOLD` (fill: info) -> `approved: false`.
- The hail typewriter restarts when `active_hail`'s key changes (`hail_tw` cache). Hails have no close button (they demand an answer, like the modals; the fail-closed timeout still runs controller-side).
- Delete `escalation_modal` + `merge_modal` and their `draw_in` calls; `hail_dialogue` takes their slot. Extend `any_dialogue_open` so the HAIL button hides while a hail is up.

- [ ] **Step 3: Gates + manual smoke** (open each hail via a real mission if feasible, else rely on the queue test + visual check of the captain dialogue reuse).

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-app/src
git commit -m "feat(app): escalation and merge hails as dialogue boxes"
```

---

### Task 7: Banners, sweep, and the retirement of Apple-minimal

**Files:**
- Modify: `crates/bridge-app/src/ui.rs` (`paused_banner` ~227, `compat_banner` ~288; dead-helper sweep)
- Modify: anything the sweep flags.

- [ ] **Step 1: Banners.** Restyle both banners as slim pixel strips: `egui::Panel::top` frame fill `theme::tint(t.warn, t.bg, 0.15)` (paused) / `tint(t.crit, t.bg, 0.15)` (compat), 1 px bottom stroke in the tint color, label in `pixel(9.0)` colored `t.warn`/`t.crit`, detail text `crt(17.0)` `t.text`, action buttons -> `pixel_button`. Behavior and commands unchanged.

- [ ] **Step 2: Sweep.** Remove the `#![allow(dead_code)]` staging comments from pixel.rs/dialogue.rs; delete now-dead helpers the compiler/clippy flag (`pill` if nothing uses it after the banner restyle, old card/section_label IF unused - check: workstream_body and captain scrollback may still use them; delete only true dead code). Grep for `ThemeMode` (must be zero). `cargo test --workspace && cargo build --workspace 2>&1` (zero warnings) `&& cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`.

- [ ] **Step 3: Launch smoke + pixel-pass checklist.** Release-build; launch in a scratch repo; walk: HAIL -> dialogue -> type -> proposal card -> MAKE IT SO -> windows via deck clicks -> banners (force compat by editing tested_version_max in a scratch bridge.toml if quick). Write a short pixel-pass checklist (dialogue chrome, portrait palettes, chip legibility at 8pt, CRT text sizes, banner tint) into the report.

- [ ] **Step 4: Commit + push**

```bash
git add -A && git commit -m "feat(app): pixel banners; Apple-minimal fully retired"
git push -u origin game-idiom
```

---

## Self-Review Notes

- Spec coverage: fonts (T1), theme (T1), console chrome + five windows (T2, T3), dialogue component/typewriter/portraits (T4), Captain dialogue + composer retirement + HAIL button (T5), hails + modal retirement (T6), banners + full retirement sweep (T7). Reduce-motion honored in Typewriter (T4) and cursor. Testing section's pure units all have tests (typewriter, chip mapping, hail queue, tw restart, portrait maps, theme tokens/fonts).
- Deviation from spec noted: the spec's "two small front-bust maps" for Tactical/Helm is implemented as palette swaps of the one PORTRAIT map (`PortraitSpeaker`) - visually equivalent, zero new art risk; flagged here deliberately.
- Type consistency: `dialogue_box` signature consumed in T5/T6 as defined in T4; `DialogueTextures` introduced in T5 and reused in T6; `chip`/`pixel_button`/`console_frame` names stable across T2-T7.
- egui 0.35 escape hatches included where the API is version-fragile (FontData insertion, rect_stroke, families assertion).
