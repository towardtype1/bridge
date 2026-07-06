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

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    #[test]
    fn tokens_differ_by_mode() {
        assert_eq!(
            tokens(ThemeMode::Light).accent,
            Color32::from_rgb(0x00, 0x71, 0xE3)
        );
        assert_eq!(
            tokens(ThemeMode::Dark).accent,
            Color32::from_rgb(0x0A, 0x84, 0xFF)
        );
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
