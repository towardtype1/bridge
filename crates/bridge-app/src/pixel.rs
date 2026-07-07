//! Reusable pixel-chrome widgets: console frames, station headers, chips,
//! and pixel buttons. Pure presentation over theme::Tokens.

use crate::theme::{self, Tokens};
use eframe::egui;
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
        egui::Rect::from_min_max(
            egui::pos2(r.min.x, r.max.y),
            egui::pos2(r.max.x, r.max.y + 2.0),
        ),
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

/// The mockup's outer hairline, 5 px outside the frame.
pub fn double_outline(ui: &mut egui::Ui, t: &Tokens) {
    let r = ui.max_rect().expand(5.0);
    ui.painter().rect_stroke(
        r,
        0.0,
        egui::Stroke::new(1.0, t.hair),
        egui::StrokeKind::Outside,
    );
}

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
