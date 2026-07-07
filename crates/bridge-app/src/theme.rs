//! The single dark pixel theme. One committed visual world: square corners,
//! no shadows, phosphor accents shared with the deck's sprite constants.

use crate::deck::sprites::{AMBER, CYAN, GREEN, RED};
use eframe::egui::{self, Color32};

/// Resolved palette. Semantic colours (good/warn/crit/info) are separate
/// from the accent and always used with a label.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub bg: Color32,
    pub surface: Color32,
    // No current consumer: the composer (its last reader) was retired in
    // Task 5. Kept for `Tokens`' struct-shape parity per the plan; Task 7's
    // sweep decides whether a future caller claims it or it's dropped.
    #[allow(dead_code)]
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

/// Blend `base` at `pct` (0..=1) over the opaque `over` ground, returning an
/// opaque colour. Used for tinted chip/selection backgrounds since egui has
/// no color-mix.
pub fn tint(base: Color32, over: Color32, pct: f32) -> Color32 {
    let p = pct.clamp(0.0, 1.0);
    let mix = |b: u8, o: u8| ((b as f32) * p + (o as f32) * (1.0 - p)).round() as u8;
    Color32::from_rgb(
        mix(base.r(), over.r()),
        mix(base.g(), over.g()),
        mix(base.b(), over.b()),
    )
}

/// egui-wide visuals for the pixel world: dark, square, shadowless.
pub fn visuals() -> egui::Visuals {
    let t = tokens();
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(t.text);
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
    v.window_stroke = egui::Stroke::new(2.0, t.info);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    let radius = egui::CornerRadius::ZERO;
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.inactive.corner_radius = radius;
    v.widgets.hovered.corner_radius = radius;
    v.widgets.active.corner_radius = radius;
    v.widgets.open.corner_radius = radius;
    v.menu_corner_radius = radius;
    v.window_corner_radius = radius;
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

#[allow(dead_code)] // consumed by Tasks C2-C6; remove the allow in Task C7
pub fn pixel(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("pixel".into()))
}

#[allow(dead_code)] // consumed by Tasks C2-C6; remove the allow in Task C7
pub fn crt(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("crt".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_the_pixel_palette() {
        let t = tokens();
        assert_eq!(t.bg, Color32::from_rgb(0x06, 0x08, 0x11));
        assert_eq!(t.accent, Color32::from_rgb(0xff, 0xb6, 0x48));
        assert_eq!(t.info, Color32::from_rgb(0x59, 0xc8, 0xff));
        assert_eq!(t.crit, Color32::from_rgb(0xff, 0x5a, 0x4e));
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
        // Fonts materialize on the first frame pass; render one that lays
        // out text in both families - an unknown family panics here.
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.label(egui::RichText::new("PIXEL").font(pixel(10.0)));
            ui.label(egui::RichText::new("CRT").font(crt(18.0)));
        });
        let families = ctx.fonts(|f| f.families());
        assert!(families.contains(&egui::FontFamily::Name("pixel".into())));
        assert!(families.contains(&egui::FontFamily::Name("crt".into())));
    }

    #[test]
    fn tint_blends_toward_base() {
        let base = Color32::from_rgb(200, 0, 0);
        let over = Color32::from_rgb(0, 0, 0);
        assert_eq!(tint(base, over, 0.0), over);
        assert_eq!(tint(base, over, 1.0), base);
        assert_eq!(tint(base, over, 0.5), Color32::from_rgb(100, 0, 0));
    }
}
