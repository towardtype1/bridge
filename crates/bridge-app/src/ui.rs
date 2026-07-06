//! egui panels. Rendering only: read `AppState`, emit `BridgeCommand`s.
//!
//! Layout:
//! - Header strip: mission state chip, budget bars (turns, wall clock,
//!   cost), rate-limit countdown banner (retry-at), compat warning banner
//!   with "proceed anyway", RED ALERT toggle (prominent, red when armed).
//! - Left sidebar: workstreams with status badges; click to select.
//! - Center: selected workstream detail - live agent output feed, tool
//!   calls, turn history, filed battle reports.
//! - Right: Tactical log (hook decisions, newest first, color by
//!   decision) and the merge queue with per-entry state.
//! - Modals: escalation queue (approve / deny-with-reason, countdown to
//!   fail-closed deny), merge confirmation (diff stat + summary,
//!   confirm/reject). The flagged-workstream override is a button in the
//!   selected workstream's detail header (visible while Flagged).
//! - Bottom: mission input box (objective -> StartMission), wind
//!   down / shutdown buttons.
//!
//! Theme: restrained LCARS accents (amber/salmon on dark), standard egui
//! widgets; readability beats cosplay.
//!
//! egui 0.35 note: the former TopBottomPanel/SidePanel types are unified
//! as `egui::Panel` (top/bottom/left/right constructors) and panels render
//! into a parent `Ui` rather than a `Context`. `draw` keeps its
//! context-based signature by building the root `Ui` itself, exactly as
//! `Context::run_ui` does internally.

use crate::state::{
    AppState, escalation_remaining_secs, format_duration_secs, mission_state_label, status_label,
};
use bridge_core::{
    BridgeCommand, BudgetExtension, DecisionKind, UserDecision, WorkstreamId, WorkstreamStatus,
};
use chrono::Utc;
use egui::{Color32, RichText};

/// LCARS-ish amber accent.
const AMBER: Color32 = Color32::from_rgb(0xFF, 0x99, 0x66);
const ALERT_RED: Color32 = Color32::from_rgb(0xCC, 0x33, 0x33);
const OK_GREEN: Color32 = Color32::from_rgb(0x66, 0xCC, 0x88);
const DIM_GRAY: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x99);
const TEST_BLUE: Color32 = Color32::from_rgb(0x77, 0xAA, 0xDD);

/// Draw one frame; queued commands are drained by the caller.
pub fn draw(ctx: &egui::Context, state: &mut AppState, out_commands: &mut Vec<BridgeCommand>) {
    let mut root = egui::Ui::new(
        ctx.clone(),
        egui::Id::new((ctx.viewport_id(), "bridge_root_ui")),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    draw_in(&mut root, state, out_commands);
}

/// Draw one frame into a root `Ui`.
fn draw_in(ui: &mut egui::Ui, state: &mut AppState, out_commands: &mut Vec<BridgeCommand>) {
    let ctx = ui.ctx().clone();
    apply_theme(&ctx);
    red_alert_banner(ui, state);
    header(ui, state, out_commands);
    bottom_bar(ui, state, out_commands);
    left_sidebar(ui, state);
    right_pane(ui, state);
    center_pane(ui, state, out_commands);
    escalation_modal(&ctx, state, out_commands);
    merge_confirmation_modal(&ctx, state, out_commands);

    // Countdowns need a ticking repaint even without events.
    if !state.escalations.is_empty() || state.rate_limit.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(0x0d, 0x0d, 0x15);
    visuals.window_fill = Color32::from_rgb(0x15, 0x15, 0x20);
    visuals.extreme_bg_color = Color32::from_rgb(0x08, 0x08, 0x0e);
    visuals.selection.bg_fill = Color32::from_rgb(0x66, 0x3d, 0x29);
    visuals.hyperlink_color = AMBER;
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = ALERT_RED;
    ctx.set_visuals(visuals);
}

fn red_alert_banner(ui: &mut egui::Ui, state: &AppState) {
    if !state.red_alert {
        return;
    }
    egui::Panel::top("red_alert_banner")
        .frame(egui::Frame::new().fill(ALERT_RED).inner_margin(6.0))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("RED ALERT - every tool call escalates to you")
                        .color(Color32::WHITE)
                        .strong(),
                );
            });
        });
}

fn header(ui: &mut egui::Ui, state: &mut AppState, out: &mut Vec<BridgeCommand>) {
    egui::Panel::top("header").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("BRIDGE").color(AMBER).strong().size(18.0));
            ui.separator();

            let mission_text = state
                .mission_state
                .as_ref()
                .map(mission_state_label)
                .unwrap_or_else(|| "STANDING BY".to_owned());
            ui.label(RichText::new(mission_text).color(AMBER).strong());
            if let Some(detail) = &state.mission_detail {
                ui.label(RichText::new(detail).color(DIM_GRAY));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let alert_label = if state.red_alert {
                    RichText::new("RED ALERT: ON")
                        .color(Color32::WHITE)
                        .strong()
                } else {
                    RichText::new("RED ALERT: OFF").color(ALERT_RED)
                };
                let button = egui::Button::new(alert_label).fill(if state.red_alert {
                    ALERT_RED
                } else {
                    ui.visuals().widgets.inactive.bg_fill
                });
                if ui.add(button).clicked() {
                    let next = !state.red_alert;
                    state.red_alert = next;
                    out.push(BridgeCommand::SetRedAlert(next));
                }
            });
        });

        budget_row(ui, state, out);
        rate_limit_row(ui, state);
        compat_warning_row(ui, state);
        ui.add_space(4.0);
    });
}

fn budget_row(ui: &mut egui::Ui, state: &AppState, out: &mut Vec<BridgeCommand>) {
    let Some(budget) = state.budget.clone() else {
        return;
    };
    ui.horizontal(|ui| {
        if let Some(fraction) = state.budget_fraction() {
            ui.label(RichText::new("TURNS").color(DIM_GRAY).small());
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(140.0)
                    .text(format!("{}/{}", budget.total_turns, budget.max_total_turns)),
            );
        }
        if let (Some(fraction), Some(max)) =
            (state.wall_clock_fraction(), budget.max_wall_clock_secs)
        {
            ui.label(RichText::new("CLOCK").color(DIM_GRAY).small());
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(140.0)
                    .text(format!(
                        "{} / {}",
                        format_duration_secs(budget.wall_clock_secs),
                        format_duration_secs(max)
                    )),
            );
        }
        ui.label(RichText::new(format!("COST ${:.2}", budget.total_cost_usd)).color(AMBER));
        if ui.button("+20 TURNS").clicked() {
            out.push(BridgeCommand::ExtendBudget(BudgetExtension {
                extra_total_turns: 20,
                ..BudgetExtension::default()
            }));
        }
    });
}

fn rate_limit_row(ui: &mut egui::Ui, state: &AppState) {
    if let Some(text) = state.rate_limit_countdown_text(Utc::now()) {
        ui.label(RichText::new(text).color(ALERT_RED).strong());
    }
}

fn compat_warning_row(ui: &mut egui::Ui, state: &mut AppState) {
    let Some((detected, min, max)) = state.compat_warning.clone() else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "COMPAT WARNING: claude {detected} is outside the tested range {min} - {max}"
            ))
            .color(AMBER)
            .strong(),
        );
        if ui.button("Proceed anyway").clicked() {
            state.compat_warning = None;
        }
    });
}

fn bottom_bar(ui: &mut egui::Ui, state: &mut AppState, out: &mut Vec<BridgeCommand>) {
    egui::Panel::bottom("mission_input").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("OBJECTIVE").color(AMBER).small());
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.ui.objective)
                    .desired_width((ui.available_width() - 260.0).max(80.0))
                    .hint_text("state your mission objective"),
            );
            let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let engage = ui
                .add_enabled(
                    !state.ui.objective.trim().is_empty(),
                    egui::Button::new(RichText::new("ENGAGE").color(AMBER).strong()),
                )
                .clicked();
            if (engage || submitted) && !state.ui.objective.trim().is_empty() {
                out.push(BridgeCommand::StartMission {
                    objective: state.ui.objective.trim().to_owned(),
                });
                state.ui.objective.clear();
            }
            if ui.button("WIND DOWN").clicked() {
                out.push(BridgeCommand::WindDown);
            }
            if ui.button("SHUTDOWN").clicked() {
                out.push(BridgeCommand::Shutdown);
            }
        });
        ui.add_space(4.0);
    });
}

fn left_sidebar(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::left("workstreams")
        .resizable(true)
        .default_size(200.0)
        .show(ui, |ui| {
            ui.heading(RichText::new("WORKSTREAMS").color(AMBER).size(14.0));
            ui.separator();
            let order = state.workstream_order.clone();
            let mut clicked: Option<WorkstreamId> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for id in order {
                    let status = state.workstreams.get(&id).and_then(|p| p.status.as_ref());
                    let badge = status.map(status_label).unwrap_or_else(|| "…".to_owned());
                    let color = status.map(status_color).unwrap_or(DIM_GRAY);
                    let selected = state.ui.selected == Some(id);
                    let text = format!("{} {}", short_id(&id), badge);
                    if ui
                        .selectable_label(selected, RichText::new(text).color(color))
                        .clicked()
                    {
                        clicked = Some(id);
                    }
                }
            });
            if let Some(id) = clicked {
                state.ui.selected = Some(id);
            }
        });
}

fn right_pane(ui: &mut egui::Ui, state: &AppState) {
    egui::Panel::right("tactical")
        .resizable(true)
        .default_size(320.0)
        .show(ui, |ui| {
            ui.heading(RichText::new("MERGE QUEUE").color(AMBER).size(14.0));
            ui.separator();
            if state.merge_queue.is_empty() {
                ui.label(RichText::new("empty").color(DIM_GRAY));
            }
            for entry in &state.merge_queue {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{}", entry.position)).color(AMBER));
                    ui.label(&entry.branch);
                    ui.label(
                        RichText::new(format!("{:?}", entry.state))
                            .color(DIM_GRAY)
                            .small(),
                    );
                });
            }
            ui.add_space(8.0);
            ui.heading(RichText::new("TACTICAL LOG").color(AMBER).size(14.0));
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for record in state.tactical_feed.iter().rev() {
                    let color = decision_color(record.decision);
                    let tool = record.tool_name.as_deref().unwrap_or("-");
                    ui.label(
                        RichText::new(format!(
                            "{:?} {} [{}] {}",
                            record.decision, record.hook_event, tool, record.rule
                        ))
                        .color(color)
                        .small(),
                    );
                    if let Some(reason) = &record.reason {
                        ui.label(RichText::new(reason).color(DIM_GRAY).small());
                    }
                }
            });
        });
}

fn center_pane(ui: &mut egui::Ui, state: &mut AppState, out: &mut Vec<BridgeCommand>) {
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(selected) = state.ui.selected else {
            global_log_view(ui, state);
            return;
        };
        let Some(panel) = state.workstreams.get(&selected) else {
            global_log_view(ui, state);
            return;
        };

        ui.horizontal(|ui| {
            ui.heading(RichText::new(short_id(&selected)).color(AMBER));
            if let Some(status) = &panel.status {
                ui.label(
                    RichText::new(status_label(status))
                        .color(status_color(status))
                        .strong(),
                );
                if matches!(status, WorkstreamStatus::Flagged)
                    && ui
                        .button(RichText::new("OVERRIDE FLAGGED").color(ALERT_RED).strong())
                        .clicked()
                {
                    out.push(BridgeCommand::OverrideFlagged {
                        workstream: selected,
                    });
                }
            }
        });
        ui.separator();

        let reports: Vec<_> = state
            .battle_reports
            .iter()
            .filter(|r| r.workstream == selected)
            .collect();
        if !reports.is_empty() {
            ui.label(RichText::new("BATTLE REPORTS").color(AMBER).small());
            for report in reports {
                let color = match report.verdict {
                    bridge_core::Verdict::Clean => OK_GREEN,
                    bridge_core::Verdict::Breached => ALERT_RED,
                };
                ui.label(
                    RichText::new(format!(
                        "round {}: {:?}, {} finding(s)",
                        report.round,
                        report.verdict,
                        report.findings.len()
                    ))
                    .color(color),
                );
                for finding in &report.findings {
                    ui.label(
                        RichText::new(format!(
                            "  [{:?}] {} ({})",
                            finding.severity, finding.title, finding.weakness_class
                        ))
                        .color(DIM_GRAY)
                        .small(),
                    );
                }
            }
            ui.separator();
        }

        if !panel.turns.is_empty() {
            ui.label(RichText::new("TURN HISTORY").color(AMBER).small());
            for turn in panel.turns.iter().rev().take(8) {
                let cost = turn
                    .total_cost_usd
                    .map(|c| format!(" ${c:.2}"))
                    .unwrap_or_default();
                let color = if turn.is_error { ALERT_RED } else { DIM_GRAY };
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
                    .small(),
                );
            }
            ui.separator();
        }

        ui.label(RichText::new("LIVE FEED").color(AMBER).small());
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in &panel.tool_calls {
                    ui.label(RichText::new(line).color(TEST_BLUE).small().monospace());
                }
                for (station, text) in &panel.output {
                    ui.label(RichText::new(format!("[{station}] {text}")).monospace());
                }
            });
    });
}

fn global_log_view(ui: &mut egui::Ui, state: &AppState) {
    ui.label(RichText::new("SHIP'S LOG").color(AMBER).small());
    ui.separator();
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &state.logs {
                let color = match entry.level {
                    bridge_core::LogLevel::Error => ALERT_RED,
                    bridge_core::LogLevel::Warn => AMBER,
                    _ => DIM_GRAY,
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
                    .small()
                    .monospace(),
                );
            }
        });
}

fn escalation_modal(ctx: &egui::Context, state: &mut AppState, out: &mut Vec<BridgeCommand>) {
    let Some(ticket) = state.escalations.first().cloned() else {
        return;
    };
    let more_pending = state.escalations.len() - 1;
    let mut decision: Option<UserDecision> = None;
    egui::Window::new(RichText::new("TACTICAL ESCALATION").color(AMBER).strong())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(RichText::new(&ticket.question).strong());
            if let Some(tool) = &ticket.tool_name {
                ui.label(RichText::new(format!("tool: {tool}")).color(DIM_GRAY));
            }
            ui.label(
                RichText::new(&ticket.tool_input_summary)
                    .monospace()
                    .small(),
            );
            let remaining = escalation_remaining_secs(&ticket, Utc::now());
            ui.label(
                RichText::new(format!(
                    "auto-deny in {}",
                    format_duration_secs(remaining as u64)
                ))
                .color(ALERT_RED),
            );
            ui.separator();
            let reason = state.ui.deny_reasons.entry(ticket.id).or_default();
            ui.horizontal(|ui| {
                ui.label("deny reason:");
                ui.text_edit_singleline(reason);
            });
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("APPROVE").color(OK_GREEN).strong())
                    .clicked()
                {
                    decision = Some(UserDecision::Approve);
                }
                if ui
                    .button(RichText::new("DENY").color(ALERT_RED).strong())
                    .clicked()
                {
                    decision = Some(UserDecision::Deny {
                        reason: reason.clone(),
                    });
                }
            });
            if more_pending > 0 {
                ui.label(
                    RichText::new(format!("{more_pending} more pending"))
                        .color(DIM_GRAY)
                        .small(),
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

fn merge_confirmation_modal(
    ctx: &egui::Context,
    state: &mut AppState,
    out: &mut Vec<BridgeCommand>,
) {
    let Some(proposal) = state.pending_merges.first().cloned() else {
        return;
    };
    let mut approved: Option<bool> = None;
    egui::Window::new(RichText::new("CONFIRM MERGE").color(AMBER).strong())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 40.0))
        .show(ctx, |ui| {
            ui.label(RichText::new(format!("{} -> {}", proposal.branch, proposal.target)).strong());
            ui.label(&proposal.summary);
            ui.label(RichText::new(&proposal.diff_stat).monospace().small());
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("CONFIRM MERGE").color(OK_GREEN).strong())
                    .clicked()
                {
                    approved = Some(true);
                }
                if ui
                    .button(RichText::new("REJECT").color(ALERT_RED).strong())
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

fn status_color(status: &WorkstreamStatus) -> Color32 {
    match status {
        WorkstreamStatus::Pending => DIM_GRAY,
        WorkstreamStatus::Working
        | WorkstreamStatus::Rebasing
        | WorkstreamStatus::ConflictFix
        | WorkstreamStatus::InMergeQueue => AMBER,
        WorkstreamStatus::UnderTest { .. } => TEST_BLUE,
        WorkstreamStatus::Breached { .. }
        | WorkstreamStatus::Failed { .. }
        | WorkstreamStatus::Flagged => ALERT_RED,
        WorkstreamStatus::ReadyToMerge | WorkstreamStatus::Merged => OK_GREEN,
    }
}

fn decision_color(decision: DecisionKind) -> Color32 {
    match decision {
        DecisionKind::Allow => OK_GREEN,
        DecisionKind::Deny => ALERT_RED,
        DecisionKind::Escalate => AMBER,
    }
}

/// First uuid group, enough to tell workstreams apart in the sidebar.
fn short_id(id: &WorkstreamId) -> String {
    id.to_string().chars().take(8).collect()
}

// Re-export egui through eframe for the single import point.
pub use eframe::egui;
