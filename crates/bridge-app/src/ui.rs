//! egui panels. Rendering only: read `AppState`, emit `BridgeCommand`s.
//!
//! Full-window deck (see the Migration addendum in
//! `docs/superpowers/specs/2026-07-06-16bit-deck-design.md`): the deck's
//! `CentralPanel` spans the entire remaining viewport now that the
//! Apple-minimal left sidebar and right inspector are gone.
//! - No app toolbar; the OS supplies the title bar.
//! - Center: the deck (`crate::deck::render::draw_deck`) plus five interim
//!   `egui::Window`s opened by deck clicks: workstream detail (title +
//!   status pill + "Open in VS Code" (+ Override while Flagged), a
//!   station/branch subtitle, a battle-report card, turn history, and the
//!   activity feed), Tactical (the exceptions-only Guardrails feed),
//!   Kobayashi Maru (battle-report summaries), Ship's Computer (the Ship's
//!   Log), and Mission Status (mission title/state, the merge queue, budget
//!   usage, and the Wind down / Stop commands - formerly sidebar chrome).
//! - Captain conference: hailing the Captain on the deck opens `captain_dialogue`,
//!   the shared RPG dialogue chrome (`crate::dialogue::dialogue_box`) hosting
//!   the transcript scrollback, the typewriter body, the Mission Briefing
//!   proposal card, and the "say something to the Captain" input row. A
//!   floating HAIL THE CAPTAIN button (`Area`, `CENTER_BOTTOM`) opens it when
//!   no dialogue is already open; there is no bottom composer anymore.
//! - Conditional top banners: paused (rate-limit / budget) and compat warning.
//! - Modals (`Window`): escalation and merge confirmation.
//!
//! Depth is hairline + grouped fill, never shadow (disabled in `theme`).
//! Only the mission/working dots animate, and only while work is live.
//!
//! egui 0.35 note: `Panel::{top,bottom,left,right}` render into a parent `Ui`
//! rather than a `Context`. `draw` keeps its context-based signature by
//! building the root `Ui` itself, exactly as `Context::run_ui` does.

use crate::dialogue;
use crate::pixel::{self, ChipKind};
use crate::state::{
    AppState, CaptainSpeaker, escalation_remaining_secs, format_duration_secs, mission_state_label,
    status_label,
};
use crate::theme::{self, Tokens};
use bridge_core::{
    BridgeCommand, BudgetExtension, BudgetSnapshot, DecisionKind, LogLevel, MergeQueueState,
    MissionState, PauseReason, UserDecision, Verdict, WorkstreamId, WorkstreamStatus,
};
use chrono::Utc;
use egui::{Align, Align2, Color32, Layout, RichText};

/// Draw one frame; queued commands are drained by the caller.
pub fn draw(
    ctx: &egui::Context,
    state: &mut AppState,
    deck: &mut crate::deck::render::DeckCanvas,
    textures: &mut crate::DialogueTextures,
    ui_cfg: &bridge_core::UiConfig,
    out_commands: &mut Vec<BridgeCommand>,
) {
    ctx.set_visuals(theme::visuals());
    let t = theme::tokens();
    let mut root = egui::Ui::new(
        ctx.clone(),
        egui::Id::new((ctx.viewport_id(), "bridge_root_ui")),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    draw_in(&mut root, &t, state, deck, textures, ui_cfg, out_commands);
}

/// Draw one frame into a root `Ui`. Panel order is top, bottom, left, right,
/// central, then the centered modals.
fn draw_in(
    ui: &mut egui::Ui,
    t: &Tokens,
    state: &mut AppState,
    deck: &mut crate::deck::render::DeckCanvas,
    textures: &mut crate::DialogueTextures,
    ui_cfg: &bridge_core::UiConfig,
    out: &mut Vec<BridgeCommand>,
) {
    let ctx = ui.ctx().clone();
    paused_banner(ui, t, state, out);
    compat_banner(ui, t, state);
    center(ui, t, state, deck, textures, ui_cfg, out);
    escalation_modal(&ctx, t, state, out);
    merge_modal(&ctx, t, state, out);

    // A ticking repaint keeps countdowns and the live-work pulse moving even
    // without incoming events; idle windows stay static.
    if !state.escalations.is_empty() || state.rate_limit.is_some() || mission_is_live(state) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

fn mission_is_live(state: &AppState) -> bool {
    matches!(state.mission_state, Some(MissionState::Executing))
}

/// Translate a deck click into a state change. Pure and unit-tested; the
/// deck's own hit-testing decides which action (if any) a click produces.
pub fn apply_deck_action(state: &mut AppState, action: crate::deck::DeckAction) {
    use crate::deck::DeckAction as A;
    match action {
        A::SelectWorkstream(id) => state.ui.selected = Some(id),
        A::HailCaptain => state.ui.open_captain = true,
        A::OpenTactical => state.ui.open_tactical = true,
        A::OpenKobayashi => state.ui.open_kobayashi = true,
        A::OpenComputer => state.ui.open_computer = true,
        A::OpenMissionStatus => state.ui.open_mission_status = true,
    }
}

// -- helper widgets -----------------------------------------------------------

/// A grouped-inset card: a filled rounded container with a hairline border and
/// no shadow (the macOS System Settings look).
fn card<R>(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(t.surface)
        .stroke(egui::Stroke::new(1.0, t.hair_2))
        .corner_radius(13)
        .inner_margin(egui::Margin::symmetric(20, 18))
        .show(ui, add)
        .inner
}

/// Uppercase tertiary section header used across the interim windows and
/// content areas.
fn section_label(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .color(t.text_3)
            .size(11.0)
            .strong(),
    );
}

/// A 9px status dot. When `pulse` is set (live/working) it soft-pulses and
/// asks for a fast repaint, so idle windows never spin.
fn dot(ui: &mut egui::Ui, color: Color32, pulse: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    let color = if pulse {
        let phase = (ui.input(|i| i.time) * std::f64::consts::TAU / 2.0).sin() as f32; // -1..1
        let alpha = 0.55 + 0.45 * (0.5 + 0.5 * phase); // 0.55..1.0
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(40));
        color.gamma_multiply(alpha)
    } else {
        color
    };
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 4.5, color);
    }
    resp
}

/// Workstream status -> chip kind for the workstream window's status chip.
fn status_chip_kind(status: &WorkstreamStatus) -> ChipKind {
    match status {
        WorkstreamStatus::Working => ChipKind::Warn,
        WorkstreamStatus::Merged | WorkstreamStatus::ReadyToMerge => ChipKind::Good,
        WorkstreamStatus::Breached { .. }
        | WorkstreamStatus::Failed { .. }
        | WorkstreamStatus::Flagged => ChipKind::Crit,
        WorkstreamStatus::UnderTest { .. } => ChipKind::Info,
        WorkstreamStatus::Pending
        | WorkstreamStatus::Rebasing
        | WorkstreamStatus::ConflictFix
        | WorkstreamStatus::InMergeQueue
        | WorkstreamStatus::Cancelled => ChipKind::Info,
    }
}

fn mission_state_color(state: &AppState, t: &Tokens) -> Color32 {
    match &state.mission_state {
        Some(MissionState::Executing) => t.accent,
        Some(MissionState::Paused { .. }) => t.warn,
        Some(MissionState::Complete) => t.good,
        Some(MissionState::Failed { .. }) => t.crit,
        _ => t.text_3,
    }
}

/// Guardrail decision -> chip kind for the tactical window's guardrail rows.
fn decision_chip_kind(decision: DecisionKind) -> ChipKind {
    match decision {
        DecisionKind::Allow => ChipKind::Allow,
        DecisionKind::Deny => ChipKind::Deny,
        DecisionKind::Escalate => ChipKind::Escalate,
    }
}

fn decision_label(decision: DecisionKind) -> &'static str {
    match decision {
        DecisionKind::Allow => "Allow",
        DecisionKind::Deny => "Deny",
        DecisionKind::Escalate => "Escalate",
    }
}

/// Battle-report verdict -> chip kind for the kobayashi and workstream
/// windows' verdict chips.
fn verdict_chip_kind(verdict: Verdict) -> ChipKind {
    match verdict {
        Verdict::Clean => ChipKind::Good,
        Verdict::Breached => ChipKind::Crit,
    }
}

fn merge_state_label(state: MergeQueueState) -> &'static str {
    match state {
        MergeQueueState::AwaitingRebase => "Awaiting rebase",
        MergeQueueState::Rebasing => "Rebasing",
        MergeQueueState::ConflictFix => "Conflict fix",
        MergeQueueState::ChecksRunning => "Checks running",
        MergeQueueState::AwaitingConfirmation => "Awaiting confirmation",
        MergeQueueState::Merging => "Merging",
        MergeQueueState::Done => "Merged",
    }
}

/// First uuid group, enough to tell workstreams apart in the interim
/// windows.
fn short_id(id: &WorkstreamId) -> String {
    id.to_string().chars().take(8).collect()
}

// -- banners ------------------------------------------------------------------

fn paused_banner(ui: &mut egui::Ui, t: &Tokens, state: &AppState, out: &mut Vec<BridgeCommand>) {
    let Some(MissionState::Paused { reason }) = state.mission_state.clone() else {
        return;
    };
    let frame = egui::Frame::new()
        .fill(theme::tint(t.warn, t.bg, 0.14))
        .inner_margin(egui::Margin::symmetric(20, 10));
    egui::Panel::top("paused_banner")
        .frame(frame)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                dot(ui, t.warn, false);
                ui.add_space(6.0);
                match &reason {
                    PauseReason::RateLimited { retry_at } => {
                        let text =
                            state
                                .rate_limit_countdown_text(Utc::now())
                                .unwrap_or_else(|| match retry_at {
                                    Some(at) => {
                                        format!(
                                            "Rate limited - retry at {}",
                                            at.format("%H:%M:%S UTC")
                                        )
                                    }
                                    None => "Rate limited - waiting for reset".to_owned(),
                                });
                        ui.label(RichText::new(text).color(t.warn).strong());
                    }
                    PauseReason::BudgetExhausted { which } => {
                        ui.label(
                            RichText::new(format!("Paused - budget exhausted ({which})"))
                                .color(t.warn)
                                .strong(),
                        );
                        ui.add_space(10.0);
                        if ui
                            .add(
                                egui::Button::new(RichText::new("+20 turns").color(t.accent))
                                    .fill(t.fill),
                            )
                            .clicked()
                        {
                            out.push(BridgeCommand::ExtendBudget(BudgetExtension {
                                extra_total_turns: 20,
                                ..BudgetExtension::default()
                            }));
                        }
                    }
                    PauseReason::UserRequested => {
                        ui.label(
                            RichText::new("Paused - user requested")
                                .color(t.warn)
                                .strong(),
                        );
                    }
                }
            });
        });
}

fn compat_banner(ui: &mut egui::Ui, t: &Tokens, state: &mut AppState) {
    let Some((detected, min, max)) = state.compat_warning.clone() else {
        return;
    };
    let frame = egui::Frame::new()
        .fill(theme::tint(t.warn, t.bg, 0.14))
        .inner_margin(egui::Margin::symmetric(20, 10));
    egui::Panel::top("compat_banner")
        .frame(frame)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                dot(ui, t.warn, false);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "claude {detected} is outside the tested range {min} - {max}"
                    ))
                    .color(t.warn)
                    .strong(),
                );
                ui.add_space(10.0);
                if ui
                    .add(egui::Button::new("Proceed anyway").fill(t.fill))
                    .clicked()
                {
                    state.compat_warning = None;
                }
            });
        });
}

// -- mission status -----------------------------------------------------------

/// Title text for the mission header: the objective while planning, else the
/// state name; the dot's tooltip always carries the precise state.
fn mission_title(state: &AppState) -> String {
    if let Some(detail) = &state.mission_detail {
        return detail.clone();
    }
    match &state.mission_state {
        Some(ms) => mission_state_label(ms),
        None => "No active mission".to_owned(),
    }
}

fn merge_queue_group(ui: &mut egui::Ui, t: &Tokens, state: &AppState) {
    section_label(ui, t, "Merge queue");
    ui.add_space(8.0);
    if state.merge_queue.is_empty() {
        ui.label(RichText::new("Empty").color(t.text_3).size(12.0));
    }
    for entry in &state.merge_queue {
        ui.horizontal(|ui| {
            number_chip(ui, t, entry.position);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    RichText::new(&entry.branch)
                        .color(t.text)
                        .size(12.5)
                        .monospace(),
                );
                let done = matches!(entry.state, MergeQueueState::Done);
                let color = if done { t.good } else { t.text_2 };
                ui.label(
                    RichText::new(merge_state_label(entry.state))
                        .color(color)
                        .size(11.0),
                );
            });
        });
        ui.add_space(6.0);
    }
}

fn number_chip(ui: &mut egui::Ui, t: &Tokens, position: u32) {
    egui::Frame::new()
        .fill(t.fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("{}", position + 1))
                    .color(t.text_2)
                    .size(11.0)
                    .monospace(),
            );
        });
}

// -- guardrails (the Tactical window) -----------------------------------------

fn guardrails_group(ui: &mut egui::Ui, t: &Tokens, state: &mut AppState) {
    let total = state.tactical_feed.len();
    let clear = state
        .tactical_feed
        .iter()
        .filter(|r| r.decision == DecisionKind::Allow)
        .count();

    ui.horizontal(|ui| {
        section_label(ui, t, "Guardrails");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{total} checks - {clear} clear"))
                    .color(t.text_3)
                    .size(11.0),
            );
        });
    });
    ui.add_space(8.0);

    let show_all = state.ui.show_all_guardrails;
    let mut shown = 0usize;
    for record in state.tactical_feed.iter().rev() {
        if record.decision == DecisionKind::Allow && !show_all {
            continue;
        }
        shown += 1;
        guardrail_row(ui, t, record);
    }
    if shown == 0 {
        let msg = if show_all {
            "No activity yet"
        } else {
            "No exceptions"
        };
        ui.label(RichText::new(msg).color(t.text_3).size(12.0));
    }

    ui.add_space(8.0);
    let link_text = if show_all {
        "Hide routine activity"
    } else {
        "Show all activity"
    };
    if ui
        .link(RichText::new(link_text).color(t.accent).size(12.0))
        .clicked()
    {
        state.ui.show_all_guardrails = !state.ui.show_all_guardrails;
    }
}

fn guardrail_row(ui: &mut egui::Ui, t: &Tokens, record: &bridge_core::HookDecisionRecord) {
    ui.horizontal(|ui| {
        pixel::chip(
            ui,
            t,
            decision_label(record.decision),
            decision_chip_kind(record.decision),
        );
        ui.add_space(8.0);
        let label = record.tool_name.as_deref().unwrap_or(record.rule.as_str());
        ui.label(RichText::new(label).color(t.text).size(12.5).monospace());
    });
    if let Some(reason) = &record.reason {
        ui.label(RichText::new(reason).color(t.text_3).size(11.0));
    }
    ui.add_space(8.0);
}

// -- content ------------------------------------------------------------------

/// The deck fills the entire center panel; the selected workstream,
/// tactical, Kobayashi, computer, and viewscreen (mission status) consoles
/// each open a stock `egui::Window` on top of it. These five windows are
/// interim: faithful to the previous flat layout, not yet restyled for the
/// deck (a later task covers that). The Captain conference is not a window
/// at all: it's the RPG dialogue box, anchored to the bottom of the deck's
/// own rect.
fn center(
    ui: &mut egui::Ui,
    t: &Tokens,
    state: &mut AppState,
    deck: &mut crate::deck::render::DeckCanvas,
    textures: &mut crate::DialogueTextures,
    ui_cfg: &bridge_core::UiConfig,
    out: &mut Vec<BridgeCommand>,
) {
    let frame = egui::Frame::new().fill(egui::Color32::from_rgb(0x06, 0x08, 0x11));
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        if let Some(action) = crate::deck::render::draw_deck(
            deck,
            ui,
            state,
            ui_cfg.deck_palette,
            ui_cfg.reduce_motion,
        ) {
            apply_deck_action(state, action);
        }
    });
    let ctx = ui.ctx().clone();
    workstream_window(&ctx, t, state, out);
    tactical_window(&ctx, t, state);
    kobayashi_window(&ctx, t, state);
    computer_window(&ctx, t, state);
    mission_status_window(&ctx, t, state, out);

    let deck_rect = deck
        .last_rect
        .unwrap_or_else(|| dialogue::fallback_deck_rect(&ctx));
    captain_dialogue(&ctx, t, state, textures, deck_rect, ui_cfg, out);

    if !any_dialogue_open(state) {
        egui::Area::new(egui::Id::new("hail_button"))
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
            .show(&ctx, |ui| {
                if pixel::pixel_button(ui, t, "HAIL THE CAPTAIN", t.accent).clicked() {
                    state.ui.open_captain = true;
                }
            });
    }
}

/// Whether any RPG dialogue is currently open: gates the floating HAIL
/// button so it never competes with an open dialogue for the same
/// bottom-anchored screen space. Task 6 extends this with the Tactical and
/// Helm hail dialogues.
fn any_dialogue_open(state: &AppState) -> bool {
    state.ui.open_captain
}

/// Selecting a console on the deck opens this window; closing it (the
/// title-bar X) clears the selection so the deck stops highlighting it.
fn workstream_window(
    ctx: &egui::Context,
    t: &Tokens,
    state: &mut AppState,
    out: &mut Vec<BridgeCommand>,
) {
    let Some(selected) = state.ui.selected else {
        return;
    };
    let mut open = true;
    egui::Window::new("workstream_console")
        .title_bar(false)
        .frame(pixel::console_frame(t))
        .default_size(egui::vec2(560.0, 520.0))
        .open(&mut open)
        .show(ctx, |ui| {
            pixel::double_outline(ui, t);
            ui.horizontal(|ui| {
                pixel::station_header(ui, t, &format!("Helm console - {}", short_id(&selected)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pixel::pixel_button(ui, t, "CLOSE", t.info).clicked() {
                        state.ui.selected = None;
                    }
                });
            });
            ui.add_space(8.0);
            workstream_body(ui, t, state, selected, out);
        });
    if !open {
        state.ui.selected = None;
    }
}

/// The workstream detail body: title row, status pill, "Open in VS Code"
/// (+ Override while Flagged), station/branch subtitle, battle-report card,
/// turn history, and the activity feed. Unchanged from the pre-deck center
/// panel, just re-hosted inside a `Window` instead of the `CentralPanel`.
fn workstream_body(
    ui: &mut egui::Ui,
    t: &Tokens,
    state: &AppState,
    selected: WorkstreamId,
    out: &mut Vec<BridgeCommand>,
) {
    let Some(panel) = state.workstreams.get(&selected) else {
        ui.label(
            RichText::new("Workstream not found")
                .color(t.text_3)
                .size(12.5),
        );
        return;
    };

    // Owned copies so the interactive closures below don't borrow `state`.
    let status = panel.status.clone();
    let worktree_path = panel.worktree_path.clone();
    let editor_command = state.ui.editor_command.clone();

    // Title row: name + status pill + right-aligned actions.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(short_id(&selected))
                .color(t.text)
                .size(26.0)
                .strong(),
        );
        ui.add_space(10.0);
        if let Some(status) = &status {
            pixel::chip(ui, t, &status_label(status), status_chip_kind(status));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let vscode_enabled = worktree_path.is_some();
            let vscode = ui
                .add_enabled_ui(vscode_enabled, |ui| {
                    pixel::pixel_button(ui, t, "OPEN IN VS CODE", t.info)
                })
                .inner;
            if vscode.clicked()
                && let Some(path) = &worktree_path
            {
                spawn_editor(&editor_command, path);
            }
            if matches!(status, Some(WorkstreamStatus::Flagged)) {
                ui.add_space(8.0);
                if pixel::pixel_button(ui, t, "OVERRIDE", t.crit).clicked() {
                    out.push(BridgeCommand::OverrideFlagged {
                        workstream: selected,
                    });
                }
            }
        });
    });

    // Subtitle: station and branch, mono; short id when neither is known.
    let station = panel.turns.last().map(|turn| turn.station.to_string());
    let branch = state
        .merge_queue
        .iter()
        .find(|e| e.workstream == selected)
        .map(|e| e.branch.clone())
        .or_else(|| {
            state
                .pending_merges
                .iter()
                .find(|p| p.workstream == selected)
                .map(|p| p.branch.clone())
        });
    let subtitle = match (station, branch) {
        (Some(s), Some(b)) => format!("{s} · {b}"),
        (Some(s), None) => s,
        (None, Some(b)) => b,
        (None, None) => short_id(&selected),
    };
    ui.add_space(2.0);
    ui.label(
        RichText::new(subtitle)
            .color(t.text_3)
            .size(12.0)
            .monospace(),
    );
    ui.add_space(16.0);

    battle_report_card(ui, t, state, selected);
    turn_history(ui, t, panel);

    ui.add_space(16.0);
    section_label(ui, t, "Activity");
    ui.add_space(8.0);
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for line in &panel.tool_calls {
                ui.label(
                    RichText::new(line)
                        .color(t.info)
                        .size(12.0)
                        .monospace()
                        .font(theme::crt(17.0)),
                );
            }
            for (station, text) in &panel.output {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(station.to_string())
                            .color(t.text_2)
                            .size(12.5)
                            .monospace()
                            .font(theme::crt(17.0)),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(text)
                            .color(t.text)
                            .size(13.5)
                            .font(theme::crt(17.0)),
                    );
                });
            }
        });
}

/// The tactical console: the exceptions-only Guardrails feed (denials and
/// escalations, with a "Show all activity" toggle).
fn tactical_window(ctx: &egui::Context, t: &Tokens, state: &mut AppState) {
    let mut open = state.ui.open_tactical;
    if !open {
        return;
    }
    egui::Window::new("tactical_console")
        .title_bar(false)
        .frame(pixel::console_frame(t))
        .default_size(egui::vec2(520.0, 420.0))
        .open(&mut open)
        .show(ctx, |ui| {
            pixel::double_outline(ui, t);
            ui.horizontal(|ui| {
                pixel::station_header(ui, t, "Tactical - guardrail adjudications");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pixel::pixel_button(ui, t, "CLOSE", t.info).clicked() {
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

/// The Kobayashi Maru console: a plain summary list (workstream, verdict,
/// finding count) per battle report, newest last.
fn kobayashi_window(ctx: &egui::Context, t: &Tokens, state: &mut AppState) {
    let mut open = state.ui.open_kobayashi;
    if !open {
        return;
    }
    egui::Window::new("kobayashi_console")
        .title_bar(false)
        .frame(pixel::console_frame(t))
        .default_size(egui::vec2(520.0, 420.0))
        .open(&mut open)
        .show(ctx, |ui| {
            pixel::double_outline(ui, t);
            ui.horizontal(|ui| {
                pixel::station_header(ui, t, "Kobayashi Maru - battle report");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pixel::pixel_button(ui, t, "CLOSE", t.info).clicked() {
                        state.ui.open_kobayashi = false;
                    }
                });
            });
            ui.add_space(8.0);
            kobayashi_summary(ui, t, state);
        });
    if !open {
        state.ui.open_kobayashi = false;
    }
}

fn kobayashi_summary(ui: &mut egui::Ui, t: &Tokens, state: &AppState) {
    if state.battle_reports.is_empty() {
        ui.label(
            RichText::new("No battle reports yet")
                .color(t.text_3)
                .size(12.0),
        );
        return;
    }
    for report in &state.battle_reports {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(short_id(&report.workstream))
                    .color(t.text)
                    .size(12.5)
                    .monospace(),
            );
            ui.add_space(8.0);
            let verdict_text = match report.verdict {
                Verdict::Clean => "Clean",
                Verdict::Breached => "Breached",
            };
            pixel::chip(ui, t, verdict_text, verdict_chip_kind(report.verdict));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} finding(s)", report.findings.len()))
                        .color(t.text_3)
                        .size(12.0),
                );
            });
        });
        ui.add_space(6.0);
    }
}

/// The computer console: the Ship's Log, unchanged.
fn computer_window(ctx: &egui::Context, t: &Tokens, state: &mut AppState) {
    let mut open = state.ui.open_computer;
    if !open {
        return;
    }
    egui::Window::new("computer_console")
        .title_bar(false)
        .frame(pixel::console_frame(t))
        .default_size(egui::vec2(520.0, 420.0))
        .open(&mut open)
        .show(ctx, |ui| {
            pixel::double_outline(ui, t);
            ui.horizontal(|ui| {
                pixel::station_header(ui, t, "Ship's computer - log");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pixel::pixel_button(ui, t, "CLOSE", t.info).clicked() {
                        state.ui.open_computer = false;
                    }
                });
            });
            ui.add_space(8.0);
            ships_log(ui, t, state);
        });
    if !open {
        state.ui.open_computer = false;
    }
}

/// The viewscreen: mission title/state, the merge queue, budget usage, and
/// the Wind down / Stop commands - homed here now that the sidebar (which
/// used to carry them via `mission_menu`) is gone.
fn mission_status_window(
    ctx: &egui::Context,
    t: &Tokens,
    state: &mut AppState,
    out: &mut Vec<BridgeCommand>,
) {
    let mut open = state.ui.open_mission_status;
    if !open {
        return;
    }
    egui::Window::new("mission_status_console")
        .title_bar(false)
        .frame(pixel::console_frame(t))
        .default_size(egui::vec2(340.0, 420.0))
        .open(&mut open)
        .show(ctx, |ui| {
            pixel::double_outline(ui, t);
            ui.horizontal(|ui| {
                pixel::station_header(ui, t, "Mission status");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pixel::pixel_button(ui, t, "CLOSE", t.info).clicked() {
                        state.ui.open_mission_status = false;
                    }
                });
            });
            ui.add_space(8.0);
            mission_status_body(ui, t, state, out);
        });
    if !open {
        state.ui.open_mission_status = false;
    }
}

fn mission_status_body(
    ui: &mut egui::Ui,
    t: &Tokens,
    state: &AppState,
    out: &mut Vec<BridgeCommand>,
) {
    ui.horizontal(|ui| {
        let resp = dot(ui, mission_state_color(state, t), mission_is_live(state));
        if let Some(ms) = &state.mission_state {
            resp.on_hover_text(mission_state_label(ms));
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(mission_title(state))
                .color(t.text)
                .size(15.0)
                .strong(),
        );
    });
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        if pixel::pixel_button(ui, t, "WIND DOWN", t.info).clicked() {
            out.push(BridgeCommand::WindDown);
        }
        if pixel::pixel_button(ui, t, "STOP", t.crit).clicked() {
            out.push(BridgeCommand::Shutdown);
        }
    });
    ui.add_space(16.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            merge_queue_group(ui, t, state);
            if let Some(budget) = &state.budget {
                ui.add_space(20.0);
                budget_group(ui, t, budget);
            }
        });
}

/// Simple turns-used/max and cost-so-far rows; shown only once a
/// `BudgetSnapshot` has arrived.
fn budget_group(ui: &mut egui::Ui, t: &Tokens, budget: &BudgetSnapshot) {
    section_label(ui, t, "Budget");
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "{} / {} turns",
            budget.total_turns, budget.max_total_turns
        ))
        .color(t.text_2)
        .size(12.5)
        .monospace(),
    );
    if budget.total_cost_usd > 0.0 {
        ui.label(
            RichText::new(format!("${:.2}", budget.total_cost_usd))
                .color(t.text_2)
                .size(12.5)
                .monospace(),
        );
    }
}

/// Hailing the Captain from the deck opens this RPG dialogue: a compact
/// scrollback of earlier conference lines, the Captain's typewriter body (a
/// static "Standing by, sir." before the first `CaptainSays`), the Mission
/// Briefing proposal card when one is pending, and the "say something to
/// the Captain" input row. Replaces `captain_window`/`captain_view`; the
/// proposal-card rendering that lived in `captain_view` now lives in this
/// dialogue's `add_contents`.
fn captain_dialogue(
    ctx: &egui::Context,
    t: &Tokens,
    state: &mut AppState,
    textures: &mut crate::DialogueTextures,
    deck_rect: egui::Rect,
    ui_cfg: &bridge_core::UiConfig,
    out: &mut Vec<BridgeCommand>,
) {
    if !state.ui.open_captain {
        return;
    }
    let now = ctx.input(|i| i.time);
    let reduce_motion = ui_cfg.reduce_motion;

    // The typewriter body: the last Captain line, restarted by
    // `sync_captain_tw` whenever a genuinely new one arrives; a static,
    // already-`complete()`d placeholder before the first `CaptainSays`.
    // `tw_source_idx` records which `captain_feed` entry `tw` is revealing,
    // so the scrollback below can skip it (it's shown live, not "earlier").
    let cached = state.sync_captain_tw(now);
    let tw_source_idx = cached.map(|c| c.feed_len - 1);
    let tw: dialogue::Typewriter = match cached {
        Some(cached) => cached.tw.clone(),
        None => {
            let mut placeholder = dialogue::Typewriter::new("Standing by, sir.".into(), now);
            placeholder.complete();
            placeholder
        }
    };

    let resp = dialogue::dialogue_box(
        ctx,
        t,
        "captain_dialogue",
        dialogue::Speaker::Captain,
        &tw,
        now,
        reduce_motion,
        deck_rect,
        &mut textures.captain,
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt("captain_feed_scrollback")
                .max_height(120.0)
                .auto_shrink([false, true])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for (i, (speaker, text)) in state.captain_feed.iter().enumerate() {
                        if Some(i) == tw_source_idx {
                            continue;
                        }
                        let (name, color) = match speaker {
                            CaptainSpeaker::You => ("You", t.text_2),
                            CaptainSpeaker::Captain => ("Captain", t.accent),
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(name).font(theme::crt(16.0)).color(color));
                            ui.add_space(4.0);
                            ui.label(RichText::new(text).font(theme::crt(16.0)).color(t.text_2));
                        });
                    }
                });

            if let Some(card_data) = &state.latest_proposal {
                let revision = card_data.revision;
                let header = if card_data.diff.is_some() {
                    "PLAN AMENDMENT - DIFF vs CURRENT".to_owned()
                } else {
                    format!("MISSION BRIEFING - REVISION {revision}")
                };
                ui.add_space(8.0);
                egui::Frame::new()
                    .stroke(egui::Stroke::new(
                        2.0,
                        theme::tint(t.accent, t.surface, 0.6),
                    ))
                    .fill(theme::tint(t.accent, t.surface, 0.05))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(header)
                                .font(theme::pixel(9.0))
                                .color(t.accent),
                        );
                        ui.add_space(6.0);
                        for ws in &card_data.plan.workstreams {
                            let (marker, color) = match &card_data.diff {
                                Some(d) if d.added.contains(&ws.slug) => ("+", t.good),
                                Some(d) if d.revised.contains(&ws.slug) => ("~", t.warn),
                                _ => (">", t.text_3),
                            };
                            ui.label(
                                RichText::new(format!("{marker} {}  {}", ws.slug, ws.title))
                                    .font(theme::crt(17.0))
                                    .color(color),
                            );
                        }
                        if let Some(d) = &card_data.diff {
                            for slug in &d.removed {
                                ui.label(
                                    RichText::new(format!("x {slug}  (cancelled)"))
                                        .font(theme::crt(17.0))
                                        .color(t.crit),
                                );
                            }
                        }
                        ui.add_space(8.0);
                        if pixel::pixel_button(ui, t, "MAKE IT SO", t.good).clicked() {
                            out.push(BridgeCommand::ApproveProposal { revision });
                        }
                    });
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let send_w = 56.0;
                let text_resp = ui.add(
                    egui::TextEdit::singleline(&mut state.ui.objective)
                        .font(theme::crt(18.0))
                        .desired_width((ui.available_width() - send_w - 8.0).max(60.0))
                        .hint_text("Say something to the Captain..."),
                );
                let mut submit =
                    text_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if pixel::pixel_button(ui, t, "SEND", t.info).clicked() {
                    submit = true;
                }
                if submit && !state.ui.objective.trim().is_empty() {
                    out.push(BridgeCommand::SayToCaptain {
                        text: state.ui.objective.trim().to_owned(),
                    });
                    state.ui.objective.clear();
                }
            });
        },
    );

    if resp.text_clicked
        && let Some(cached) = state.captain_tw.as_mut()
    {
        cached.tw.complete();
    }
    if resp.closed {
        state.ui.open_captain = false;
    }
}

fn battle_report_card(ui: &mut egui::Ui, t: &Tokens, state: &AppState, selected: WorkstreamId) {
    let mut reports: Vec<_> = state
        .battle_reports
        .iter()
        .filter(|r| r.workstream == selected)
        .collect();
    reports.sort_by_key(|r| r.round);
    let Some(latest) = reports.last() else {
        return;
    };
    let earlier = reports.len().saturating_sub(1);

    card(ui, t, |ui| {
        let verdict_text = match latest.verdict {
            Verdict::Clean => "Clean",
            Verdict::Breached => "Breached",
        };
        ui.horizontal(|ui| {
            pixel::chip(ui, t, verdict_text, verdict_chip_kind(latest.verdict));
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("Round {}", latest.round))
                    .color(t.text_2)
                    .size(12.0),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} finding(s)", latest.findings.len()))
                        .color(t.text_3)
                        .size(12.0),
                );
            });
        });
        for finding in &latest.findings {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{:?} · {}", finding.severity, finding.title))
                    .color(t.text)
                    .size(13.0),
            );
            ui.label(
                RichText::new(format!(
                    "{} · {}",
                    finding.weakness_class, finding.reproduction_command
                ))
                .color(t.text_3)
                .size(11.5)
                .monospace(),
            );
        }
        if earlier > 0 {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{earlier} earlier round(s)"))
                    .color(t.text_3)
                    .size(11.0),
            );
        }
    });
}

fn turn_history(ui: &mut egui::Ui, t: &Tokens, panel: &crate::state::WorkstreamPanel) {
    if panel.turns.is_empty() {
        return;
    }
    ui.add_space(16.0);
    section_label(ui, t, "Turn history");
    ui.add_space(8.0);
    for turn in panel.turns.iter().rev().take(8) {
        let cost = turn
            .total_cost_usd
            .map(|c| format!("  ${c:.2}"))
            .unwrap_or_default();
        let color = if turn.is_error { t.crit } else { t.text_2 };
        ui.label(
            RichText::new(format!(
                "{} {} {}t {}{}",
                turn.station,
                turn.subtype,
                turn.num_turns,
                format_duration_secs(turn.duration_ms / 1000),
                cost
            ))
            .color(color)
            .size(12.0)
            .monospace(),
        );
    }
}

fn ships_log(ui: &mut egui::Ui, t: &Tokens, state: &AppState) {
    section_label(ui, t, "Ship's Log");
    ui.add_space(8.0);
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &state.logs {
                let color = match entry.level {
                    LogLevel::Error => t.crit,
                    LogLevel::Warn => t.warn,
                    _ => t.text_2,
                };
                let station = entry.station.map(|s| format!("[{s}] ")).unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "{} {}{}",
                        entry.timestamp.format("%H:%M:%S"),
                        station,
                        entry.message
                    ))
                    .color(color)
                    .size(12.0)
                    .monospace()
                    .font(theme::crt(17.0)),
                );
            }
        });
}

// -- editor -------------------------------------------------------------------

/// Spawn the configured editor on a worktree. Never panics; a missing binary
/// or spawn failure is a non-blocking log entry.
fn spawn_editor(editor_command: &str, path: &std::path::Path) {
    let cmd = if editor_command.is_empty() {
        "code"
    } else {
        editor_command
    };
    match std::process::Command::new(cmd).arg(path).spawn() {
        Ok(mut child) => {
            // Reap the child on a detached thread so it never lingers as a
            // zombie process; we don't care about its exit status.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => {
            tracing::warn!(editor = %cmd, path = %path.display(), error = %err, "failed to open editor");
        }
    }
}

// -- modals -------------------------------------------------------------------

fn window_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(t.surface)
        .stroke(egui::Stroke::new(1.0, t.hair))
        .corner_radius(13)
        .inner_margin(egui::Margin::symmetric(20, 18))
}

fn escalation_modal(
    ctx: &egui::Context,
    t: &Tokens,
    state: &mut AppState,
    out: &mut Vec<BridgeCommand>,
) {
    let Some(ticket) = state.escalations.first().cloned() else {
        return;
    };
    let more_pending = state.escalations.len() - 1;
    let mut decision: Option<UserDecision> = None;
    egui::Window::new(
        RichText::new("Escalation")
            .color(t.text)
            .size(15.0)
            .strong(),
    )
    .collapsible(false)
    .resizable(false)
    .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
    .frame(window_frame(t))
    .show(ctx, |ui| {
        ui.label(
            RichText::new(&ticket.question)
                .color(t.text)
                .size(15.0)
                .strong(),
        );
        ui.add_space(8.0);
        if let Some(tool) = &ticket.tool_name {
            ui.label(RichText::new(tool).color(t.text_2).size(12.5).monospace());
        }
        ui.label(
            RichText::new(&ticket.tool_input_summary)
                .color(t.text_3)
                .size(12.0)
                .monospace(),
        );
        ui.add_space(8.0);
        let remaining = escalation_remaining_secs(&ticket, Utc::now());
        ui.label(
            RichText::new(format!(
                "Auto-deny in {}",
                format_duration_secs(remaining as u64)
            ))
            .color(t.crit)
            .size(12.5),
        );
        ui.add_space(10.0);
        let reason = state.ui.deny_reasons.entry(ticket.id).or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Deny reason").color(t.text_2).size(12.0));
            ui.text_edit_singleline(reason);
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("Approve").color(Color32::WHITE)).fill(t.good))
                .clicked()
            {
                decision = Some(UserDecision::Approve);
            }
            if ui
                .add(egui::Button::new(RichText::new("Deny").color(Color32::WHITE)).fill(t.crit))
                .clicked()
            {
                decision = Some(UserDecision::Deny {
                    reason: reason.clone(),
                });
            }
        });
        if more_pending > 0 {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{more_pending} more pending"))
                    .color(t.text_3)
                    .size(11.0),
            );
        }
    });
    if let Some(decision) = decision {
        out.push(BridgeCommand::ResolveEscalation {
            id: ticket.id,
            decision,
        });
        state.remove_escalation(ticket.id);
    }
}

fn merge_modal(
    ctx: &egui::Context,
    t: &Tokens,
    state: &mut AppState,
    out: &mut Vec<BridgeCommand>,
) {
    let Some(proposal) = state.pending_merges.first().cloned() else {
        return;
    };
    let mut approved: Option<bool> = None;
    egui::Window::new(
        RichText::new("Confirm merge")
            .color(t.text)
            .size(15.0)
            .strong(),
    )
    .collapsible(false)
    .resizable(false)
    .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 40.0))
    .frame(window_frame(t))
    .show(ctx, |ui| {
        ui.label(
            RichText::new(format!("{} -> {}", proposal.branch, proposal.target))
                .color(t.text)
                .size(14.0)
                .monospace(),
        );
        ui.add_space(8.0);
        ui.label(RichText::new(&proposal.summary).color(t.text_2).size(13.0));
        ui.add_space(8.0);
        ui.label(
            RichText::new(&proposal.diff_stat)
                .color(t.text_3)
                .size(12.0)
                .monospace(),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("Confirm").color(Color32::WHITE))
                        .fill(t.accent),
                )
                .clicked()
            {
                approved = Some(true);
            }
            if ui
                .add(egui::Button::new(RichText::new("Reject").color(t.text)).fill(t.fill))
                .clicked()
            {
                approved = Some(false);
            }
        });
    });
    if let Some(approved) = approved {
        out.push(BridgeCommand::ConfirmMerge {
            workstream: proposal.workstream,
            approved,
        });
        state.remove_merge_proposal(proposal.workstream);
    }
}

// Re-export egui through eframe for the single import point.
pub use eframe::egui;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_actions_route() {
        let mut state = AppState::default();
        let ws = bridge_core::WorkstreamId::new();
        apply_deck_action(&mut state, crate::deck::DeckAction::SelectWorkstream(ws));
        assert_eq!(state.ui.selected, Some(ws));
        apply_deck_action(&mut state, crate::deck::DeckAction::OpenTactical);
        assert!(state.ui.open_tactical);
        apply_deck_action(&mut state, crate::deck::DeckAction::OpenKobayashi);
        assert!(state.ui.open_kobayashi);
        apply_deck_action(&mut state, crate::deck::DeckAction::OpenComputer);
        assert!(state.ui.open_computer);
        apply_deck_action(&mut state, crate::deck::DeckAction::OpenMissionStatus);
        assert!(state.ui.open_mission_status);
        // HailCaptain opens the Captain window (the deck itself never shows
        // the conference; it's a stock `egui::Window` like the others).
        apply_deck_action(&mut state, crate::deck::DeckAction::HailCaptain);
        assert!(state.ui.open_captain);
    }
}
