//! egui panels. Rendering only: read `AppState`, emit `BridgeCommand`s.
//!
//! Apple-minimal layout (see `docs/superpowers/specs/*bridge-gui-apple-minimal*`):
//! - No app toolbar; the OS supplies the title bar.
//! - Left sidebar: "Bridge" wordmark + an ellipsis-circle menu (Wind down /
//!   Stop), the mission title led by a coloured state dot, then the
//!   "Workstreams" source list (status dot + name + status subtitle).
//! - Center: empty state is the Ship's Log; a selected workstream shows its
//!   title + status pill + "Open in VS Code" (+ Override while Flagged), a
//!   station/branch subtitle, a battle-report card, turn history, and the
//!   activity feed.
//! - Right inspector: the merge queue and an exceptions-only Guardrails group
//!   (denials + escalations) with a header count and a "Show all activity"
//!   toggle.
//! - Bottom composer: a rounded field addressed to the Captain; Enter or the
//!   circular send button starts a mission.
//! - Conditional top banners: paused (rate-limit / budget) and compat warning.
//! - Modals (`Window`): escalation and merge confirmation.
//!
//! Depth is hairline + grouped fill, never shadow (disabled in `theme`).
//! Only the mission/working dots animate, and only while work is live.
//!
//! egui 0.35 note: `Panel::{top,bottom,left,right}` render into a parent `Ui`
//! rather than a `Context`. `draw` keeps its context-based signature by
//! building the root `Ui` itself, exactly as `Context::run_ui` does.

use crate::state::{
    AppState, CaptainSpeaker, escalation_remaining_secs, format_duration_secs, mission_state_label,
    status_label,
};
use crate::theme::{self, ThemeMode, Tokens};
use bridge_core::{
    BridgeCommand, BudgetExtension, DecisionKind, LogLevel, MergeQueueState, MissionState,
    PauseReason, UserDecision, Verdict, WorkstreamId, WorkstreamStatus,
};
use chrono::Utc;
use egui::{Align, Align2, Color32, Layout, RichText};

/// Draw one frame; queued commands are drained by the caller.
pub fn draw(ctx: &egui::Context, state: &mut AppState, out_commands: &mut Vec<BridgeCommand>) {
    let mode = match ctx.theme() {
        egui::Theme::Dark => ThemeMode::Dark,
        egui::Theme::Light => ThemeMode::Light,
    };
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

/// Draw one frame into a root `Ui`. Panel order is top, bottom, left, right,
/// central, then the centered modals.
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

    // A ticking repaint keeps countdowns and the live-work pulse moving even
    // without incoming events; idle windows stay static.
    if !state.escalations.is_empty() || state.rate_limit.is_some() || mission_is_live(state) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

fn mission_is_live(state: &AppState) -> bool {
    matches!(state.mission_state, Some(MissionState::Executing))
}

// -- helper widgets -----------------------------------------------------------

/// A tinted, full-round status pill: semantic colour text on a ~14% tint.
fn pill(ui: &mut egui::Ui, t: &Tokens, text: &str, color: Color32) {
    let bg = theme::tint(color, t.surface, 0.14);
    egui::Frame::new()
        .fill(bg)
        .corner_radius(255)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(11.5));
        });
}

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

/// Uppercase tertiary section header used across the sidebar, inspector,
/// and content areas.
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

fn status_color(status: &WorkstreamStatus, t: &Tokens) -> Color32 {
    match status {
        WorkstreamStatus::Pending => t.text_3,
        WorkstreamStatus::Working
        | WorkstreamStatus::Rebasing
        | WorkstreamStatus::ConflictFix
        | WorkstreamStatus::InMergeQueue => t.accent,
        WorkstreamStatus::UnderTest { .. } => t.info,
        WorkstreamStatus::Breached { .. }
        | WorkstreamStatus::Failed { .. }
        | WorkstreamStatus::Flagged => t.crit,
        WorkstreamStatus::ReadyToMerge | WorkstreamStatus::Merged => t.good,
        WorkstreamStatus::Cancelled => t.text_3,
    }
}

/// Whether a status represents work in flight (drives the pulse).
fn status_is_active(status: &WorkstreamStatus) -> bool {
    matches!(
        status,
        WorkstreamStatus::Working
            | WorkstreamStatus::Rebasing
            | WorkstreamStatus::ConflictFix
            | WorkstreamStatus::UnderTest { .. }
            | WorkstreamStatus::InMergeQueue
    )
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

fn decision_color(decision: DecisionKind, t: &Tokens) -> Color32 {
    match decision {
        DecisionKind::Allow => t.good,
        DecisionKind::Deny => t.crit,
        DecisionKind::Escalate => t.warn,
    }
}

fn decision_label(decision: DecisionKind) -> &'static str {
    match decision {
        DecisionKind::Allow => "Allow",
        DecisionKind::Deny => "Deny",
        DecisionKind::Escalate => "Escalate",
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

/// First uuid group, enough to tell workstreams apart in the sidebar.
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

// -- composer -----------------------------------------------------------------

fn bottom_composer(
    ui: &mut egui::Ui,
    t: &Tokens,
    state: &mut AppState,
    out: &mut Vec<BridgeCommand>,
) {
    let frame = egui::Frame::new()
        .fill(t.surface_2)
        .inner_margin(egui::Margin::symmetric(20, 14));
    egui::Panel::bottom("composer").frame(frame).show(ui, |ui| {
        let input_id = egui::Id::new("composer_input");
        let focused = ui.memory(|m| m.has_focus(input_id));
        let stroke = if focused {
            egui::Stroke::new(1.5, t.accent)
        } else {
            egui::Stroke::new(1.0, t.hair)
        };
        let field = egui::Frame::new()
            .fill(t.surface)
            .stroke(stroke)
            .corner_radius(15)
            .inner_margin(egui::Margin::symmetric(8, 6));

        let mut submit = false;
        field.show(ui, |ui| {
            ui.horizontal(|ui| {
                pill(ui, t, "Captain", t.accent);
                ui.add_space(8.0);
                let send_w = 28.0;
                let text_w = (ui.available_width() - send_w - 10.0).max(60.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.ui.objective)
                        .id(input_id)
                        .frame(egui::Frame::new())
                        .desired_width(text_w)
                        .hint_text("Hail the Captain..."),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                let send = ui.add_sized(
                    egui::vec2(send_w, send_w),
                    egui::Button::new(RichText::new("↑").color(Color32::WHITE).size(15.0))
                        .fill(t.accent)
                        .corner_radius(255),
                );
                if send.clicked() {
                    submit = true;
                }
            });
        });

        if submit && !state.ui.objective.trim().is_empty() {
            out.push(BridgeCommand::SayToCaptain {
                text: state.ui.objective.trim().to_owned(),
            });
            state.ui.objective.clear();
        }
    });
}

// -- sidebar ------------------------------------------------------------------

fn left_sidebar(ui: &mut egui::Ui, t: &Tokens, state: &mut AppState, out: &mut Vec<BridgeCommand>) {
    let frame = egui::Frame::new()
        .fill(t.sidebar)
        .inner_margin(egui::Margin::symmetric(14, 14));
    egui::Panel::left("sidebar")
        .exact_size(238.0)
        .resizable(false)
        .frame(frame)
        .show(ui, |ui| {
            // Mission header: wordmark + ellipsis-circle menu.
            ui.horizontal(|ui| {
                ui.label(RichText::new("Bridge").color(t.text).size(13.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    mission_menu(ui, t, out);
                });
            });
            ui.add_space(12.0);

            // Mission title led by a state dot (tooltip carries the state).
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
            ui.add_space(18.0);

            section_label(ui, t, "Workstreams");
            ui.add_space(6.0);

            let order = state.workstream_order.clone();
            let mut clicked: Option<WorkstreamId> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for id in order {
                        if workstream_row(ui, t, state, id) {
                            clicked = Some(id);
                        }
                    }
                });
            if let Some(id) = clicked {
                state.ui.selected = Some(id);
            }
        });
}

/// The ellipsis-circle menu: Wind down / Stop, kept out of persistent chrome.
fn mission_menu(ui: &mut egui::Ui, t: &Tokens, out: &mut Vec<BridgeCommand>) {
    let button = egui::Button::new(RichText::new("⋯").color(t.text_2).size(16.0)).frame(false);
    egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        if ui.button("Wind down").clicked() {
            out.push(BridgeCommand::WindDown);
        }
        if ui.button("Stop").clicked() {
            out.push(BridgeCommand::Shutdown);
        }
    });
}

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

/// One source-list row. Returns true when clicked. The selected row gets a
/// soft accent-tinted fill with normal label text.
fn workstream_row(ui: &mut egui::Ui, t: &Tokens, state: &AppState, id: WorkstreamId) -> bool {
    let status = state.workstreams.get(&id).and_then(|p| p.status.clone());
    let selected = state.ui.selected == Some(id);
    let name = short_id(&id);
    let subtitle = status
        .as_ref()
        .map(status_label)
        .unwrap_or_else(|| "…".to_owned());
    let color = status
        .as_ref()
        .map(|s| status_color(s, t))
        .unwrap_or(t.text_3);
    let pulse = status.as_ref().is_some_and(status_is_active);

    let fill = if selected {
        theme::tint(t.accent, t.sidebar, 0.14)
    } else {
        Color32::TRANSPARENT
    };
    let inner = egui::Frame::new()
        .fill(fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                dot(ui, color, pulse);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(name).color(t.text).size(13.5));
                    ui.label(RichText::new(subtitle).color(t.text_2).size(11.0));
                });
            });
        });

    let resp = inner.response.interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.add_space(2.0);
    resp.clicked()
}

// -- inspector ----------------------------------------------------------------

fn right_inspector(ui: &mut egui::Ui, t: &Tokens, state: &mut AppState) {
    let frame = egui::Frame::new()
        .fill(t.surface_2)
        .inner_margin(egui::Margin::symmetric(16, 16));
    egui::Panel::right("inspector")
        .exact_size(292.0)
        .resizable(false)
        .frame(frame)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    merge_queue_group(ui, t, state);
                    ui.add_space(20.0);
                    guardrails_group(ui, t, state);
                });
        });
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
        pill(
            ui,
            t,
            decision_label(record.decision),
            decision_color(record.decision, t),
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

fn center(ui: &mut egui::Ui, t: &Tokens, state: &AppState, out: &mut Vec<BridgeCommand>) {
    let frame = egui::Frame::new()
        .fill(t.bg)
        .inner_margin(egui::Margin::symmetric(30, 26));
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        let Some(selected) = state.ui.selected else {
            captain_view(ui, t, state, out);
            return;
        };
        let Some(panel) = state.workstreams.get(&selected) else {
            ships_log(ui, t, state);
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
                pill(ui, t, &status_label(status), status_color(status, t));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let open =
                    egui::Button::new(RichText::new("Open in VS Code").color(t.text).size(12.5))
                        .fill(t.fill);
                if ui.add_enabled(worktree_path.is_some(), open).clicked()
                    && let Some(path) = &worktree_path
                {
                    spawn_editor(&editor_command, path);
                }
                if matches!(status, Some(WorkstreamStatus::Flagged)) {
                    ui.add_space(8.0);
                    let override_btn =
                        egui::Button::new(RichText::new("Override").color(t.crit).size(12.5))
                            .fill(theme::tint(t.crit, t.bg, 0.14));
                    if ui.add(override_btn).clicked() {
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
                    ui.label(RichText::new(line).color(t.info).size(12.0).monospace());
                }
                for (station, text) in &panel.output {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(station.to_string())
                                .color(t.text_2)
                                .size(12.5)
                                .monospace(),
                        );
                        ui.add_space(4.0);
                        ui.label(RichText::new(text).color(t.text).size(13.5));
                    });
                }
            });
    });
}

/// Interim conversation surface: transcript, latest proposal card, ship's
/// log below. Sub-project C replaces this with the deck dialogue.
fn captain_view(ui: &mut egui::Ui, t: &Tokens, state: &AppState, out: &mut Vec<BridgeCommand>) {
    section_label(ui, t, "Captain");
    ui.add_space(8.0);
    if let Some(card_data) = &state.latest_proposal {
        card(ui, t, |ui| {
            ui.label(
                RichText::new(format!("Proposed plan - revision {}", card_data.revision))
                    .color(t.text)
                    .size(13.0)
                    .strong(),
            );
            ui.add_space(6.0);
            for ws in &card_data.plan.workstreams {
                let marker = match &card_data.diff {
                    Some(d) if d.added.contains(&ws.slug) => "+",
                    Some(d) if d.revised.contains(&ws.slug) => "~",
                    _ => "-",
                };
                ui.label(
                    RichText::new(format!("{marker} {}  {}", ws.slug, ws.title))
                        .color(t.text_2)
                        .size(12.5)
                        .monospace(),
                );
            }
            if let Some(d) = &card_data.diff {
                for slug in &d.removed {
                    ui.label(
                        RichText::new(format!("x {slug}  (cancelled)"))
                            .color(t.crit)
                            .size(12.5)
                            .monospace(),
                    );
                }
            }
            ui.add_space(8.0);
            let btn =
                egui::Button::new(RichText::new("Make it so").color(Color32::WHITE).size(12.5))
                    .fill(t.accent);
            if ui.add(btn).clicked() {
                out.push(BridgeCommand::ApproveProposal {
                    revision: card_data.revision,
                });
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
        let (verdict_text, verdict_color) = match latest.verdict {
            Verdict::Clean => ("Clean", t.good),
            Verdict::Breached => ("Breached", t.crit),
        };
        ui.horizontal(|ui| {
            pill(ui, t, verdict_text, verdict_color);
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
                    .monospace(),
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
