//! The RPG dialogue box: pure typewriter state plus the egui component
//! reused by the Captain conference, Tactical escalation hails, and Helm
//! merge hails.

#![allow(dead_code)] // consumed by Tasks 5-6; remove in Task 7

use crate::deck::scene::{NATIVE_H, NATIVE_W};
use crate::deck::sprites::{PORTRAIT, PortraitSpeaker, portrait_color};
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
        Self {
            text,
            started: now,
            done: false,
        }
    }

    pub fn visible(&self, now: f64, reduce_motion: bool) -> &str {
        if self.done || reduce_motion {
            return &self.text;
        }
        // The tiny epsilon absorbs float representation error at exact tick
        // boundaries (e.g. `10.024 - 10.0` lands a hair under `0.024` in
        // f64), which would otherwise truncate one char early.
        let n = ((now - self.started).max(0.0) * CHARS_PER_SEC + 1e-9) as usize;
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

/// Who is speaking in the dialogue box: sets the portrait palette, the
/// header name, and the accent color of that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Captain,
    Tactical,
    Helm,
}

impl Speaker {
    pub fn name(&self) -> &'static str {
        match self {
            Speaker::Captain => "CAPTAIN",
            Speaker::Tactical => "TACTICAL - PRIORITY HAIL",
            Speaker::Helm => "HELM",
        }
    }

    pub fn accent(&self, t: &Tokens) -> Color32 {
        match self {
            Speaker::Captain => t.accent,
            Speaker::Tactical => t.crit,
            Speaker::Helm => t.warn,
        }
    }

    fn portrait_speaker(&self) -> PortraitSpeaker {
        match self {
            Speaker::Captain => PortraitSpeaker::Command,
            Speaker::Tactical => PortraitSpeaker::Tactical,
            Speaker::Helm => PortraitSpeaker::Helm,
        }
    }
}

/// What the caller should do after a `dialogue_box` frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct DialogueResponse {
    /// The corner CLOSE button was clicked; the caller should stop showing
    /// this dialogue.
    pub closed: bool,
    /// The body text region was clicked; the caller should complete its
    /// `Typewriter` (a click-to-skip the reveal).
    pub text_clicked: bool,
}

/// Paint `speaker`'s bust into a 28x28 NEAREST texture and cache it in
/// `slot`. `slot` is caller-owned (one per speaker) rather than a global, so
/// repeated calls after the first are a cheap handle clone.
pub fn portrait_texture(
    ctx: &egui::Context,
    speaker: Speaker,
    slot: &mut Option<egui::TextureHandle>,
) -> egui::TextureHandle {
    if let Some(tex) = slot {
        return tex.clone();
    }
    let portrait_speaker = speaker.portrait_speaker();
    let width = PORTRAIT.first().map_or(0, |row| row.len());
    let mut pixels = Vec::with_capacity(width * PORTRAIT.len());
    for row in PORTRAIT {
        for &b in row.as_bytes() {
            let color = portrait_color(b, portrait_speaker)
                .map(|c| Color32::from_rgb(c.0, c.1, c.2))
                .unwrap_or(Color32::TRANSPARENT);
            pixels.push(color);
        }
    }
    let image = egui::ColorImage::new([width, PORTRAIT.len()], pixels);
    let tex = ctx.load_texture(
        format!("portrait-{}", speaker.name()),
        image,
        egui::TextureOptions::NEAREST,
    );
    *slot = Some(tex.clone());
    tex
}

/// Fallback for the deck's rendered rect, approximated from the raw
/// viewport, for use only when the caller has no actual rect yet (before the
/// deck's first paint this session). Callers normally have a real one -
/// `deck::render::DeckCanvas::last_rect`, the deck's own blit rect as
/// computed by `draw_deck` - and should prefer that: the raw viewport
/// diverges from it whenever a panel (a banner) shrinks the `CentralPanel`
/// the deck actually renders into.
pub fn fallback_deck_rect(ctx: &egui::Context) -> egui::Rect {
    let screen = ctx.viewport_rect();
    let scale = crate::deck::render::integer_scale(screen.width(), screen.height()) as f32;
    let size = egui::vec2(NATIVE_W as f32 * scale, NATIVE_H as f32 * scale);
    egui::Rect::from_center_size(screen.center(), size)
}

/// The shared RPG dialogue chrome: bottom-anchored box with a portrait,
/// speaker name, and a typewriter body. Renders the frame, portrait,
/// header, and body itself, then hands the `Ui` to `add_contents` for the
/// caller's cards/choices/inputs. Reused for the Captain conference and the
/// Tactical/Helm hails - callers wire their own buttons with
/// `pixel::pixel_button` inside `add_contents`. `deck_rect` is the deck's
/// actual rendered rect (`DeckCanvas::last_rect`, or `fallback_deck_rect`
/// before the deck's first paint) - the box's width is 94% of it, so it
/// stays sized to the pixel-art canvas rather than to raw window space.
#[allow(clippy::too_many_arguments)]
pub fn dialogue_box(
    ctx: &egui::Context,
    t: &Tokens,
    id: &str,
    speaker: Speaker,
    closable: bool,
    tw: &Typewriter,
    now: f64,
    reduce_motion: bool,
    deck_rect: egui::Rect,
    portrait_slot: &mut Option<egui::TextureHandle>,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> DialogueResponse {
    let mut response = DialogueResponse::default();
    let max_width = deck_rect.width() * 0.94;
    let texture = portrait_texture(ctx, speaker, portrait_slot);
    let done = tw.is_done(now, reduce_motion);

    egui::Area::new(egui::Id::new(id))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
        .show(ctx, |ui| {
            ui.set_max_width(max_width);
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(6, 9, 20, 245))
                .stroke(egui::Stroke::new(3.0, Color32::from_rgb(0xe8, 0xec, 0xf5)))
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.set_max_width(max_width);

                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if closable && pixel::pixel_button(ui, t, "CLOSE", t.info).clicked() {
                                response.closed = true;
                            }
                        });
                    });
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        egui::Frame::new()
                            .stroke(egui::Stroke::new(2.0, t.hair))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Image::from_texture(&texture)
                                        .fit_to_exact_size(egui::vec2(84.0, 84.0)),
                                );
                            });
                        ui.add_space(12.0);

                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(speaker.name())
                                    .font(theme::pixel(10.0))
                                    .color(speaker.accent(t)),
                            );
                            ui.add_space(6.0);

                            // `horizontal_wrapped`, not `horizontal`: a plain
                            // horizontal layout never wraps, so a long
                            // Captain line would run on as one unbroken row
                            // past the dialogue's (and the window's) edge
                            // instead of wrapping within `max_width`.
                            ui.horizontal_wrapped(|ui| {
                                let body = tw.visible(now, reduce_motion);
                                let body_resp = ui.add(
                                    egui::Label::new(
                                        RichText::new(body).font(theme::crt(19.0)).color(t.text),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                if body_resp.clicked() {
                                    response.text_clicked = true;
                                }
                                if !done && !reduce_motion && (now * 2.0) as i64 % 2 == 0 {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(6.0, 14.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, 0.0, t.text);
                                }
                            });

                            add_contents(ui);
                        });
                    });
                });
        });

    if !done {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    response
}

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

    #[test]
    fn portrait_texture_is_cached_in_the_caller_slot() {
        let ctx = egui::Context::default();
        let mut slot = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx().clone();
            let first = portrait_texture(&ctx, Speaker::Captain, &mut slot);
            assert!(slot.is_some());
            let second = portrait_texture(&ctx, Speaker::Captain, &mut slot);
            assert_eq!(first.id(), second.id());
        });
    }

    #[test]
    fn dialogue_box_renders_without_panicking_and_reports_clicks() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let t = theme::tokens();
        let tw = Typewriter::new("Standing by, sir.".into(), 0.0);
        let mut slot = None;
        let mut contents_ran = false;
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let _ = ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx().clone();
            let resp = dialogue_box(
                &ctx,
                &t,
                "test_dialogue",
                Speaker::Captain,
                true,
                &tw,
                0.0,
                true,
                rect,
                &mut slot,
                |_ui| contents_ran = true,
            );
            assert!(!resp.closed);
        });
        assert!(contents_ran);
    }
}
