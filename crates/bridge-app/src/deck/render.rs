//! Software renderer: SceneState -> Color32 framebuffer -> one NEAREST
//! egui texture, blitted at integer scale. All egui usage for the deck
//! lives in this file.

use crate::deck::DeckAction;
use crate::deck::input::{self, HitInfo};
use crate::deck::scene::{
    AgentPhase, CHAIR, COMMS, COMPUTER, KOBA, LIFT, MAX_CONSOLES, NATIVE_H, NATIVE_W, SceneState,
    TACTICAL, VIEWSCREEN, console_box,
};
use crate::deck::sprites::{
    AMBER, CHAIR_CAPT, CREW_BACK, CREW_WALK_A, CREW_WALK_B, CYAN, ConsoleVisual, CrewColors,
    CrewStation, EXCLAIM, RED, Rgb, ScenePalette, SpriteMap, crew_colors, scene_palette,
    sprite_color, visual_colors,
};
use crate::state::AppState;
use bridge_core::DeckPalette;
use eframe::egui;

pub struct DeckCanvas {
    pub scene: SceneState,
    /// The deck's actual on-screen rect from its most recent paint (the
    /// native canvas blitted at integer scale, centered in the available
    /// area). The dialogue box sizes itself off this rather than
    /// approximating one from the raw viewport, since panels (banners)
    /// shrink the `CentralPanel` the deck actually renders into. `None`
    /// only before the deck's first paint this session.
    pub last_rect: Option<egui::Rect>,
    texture: Option<egui::TextureHandle>,
    buf: Vec<egui::Color32>,
}

impl Default for DeckCanvas {
    fn default() -> Self {
        Self {
            scene: SceneState::default(),
            last_rect: None,
            texture: None,
            buf: vec![egui::Color32::BLACK; NATIVE_W * NATIVE_H],
        }
    }
}

fn c32(c: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.0, c.1, c.2)
}

fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let lerp = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Rgb(lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

pub fn integer_scale(avail_w: f32, avail_h: f32) -> u32 {
    let sx = (avail_w / NATIVE_W as f32).floor() as u32;
    let sy = (avail_h / NATIVE_H as f32).floor() as u32;
    sx.min(sy).max(1)
}

fn fill_rect(buf: &mut [egui::Color32], x: i32, y: i32, w: i32, h: i32, color: egui::Color32) {
    for yy in y.max(0)..(y + h).min(NATIVE_H as i32) {
        for xx in x.max(0)..(x + w).min(NATIVE_W as i32) {
            buf[yy as usize * NATIVE_W + xx as usize] = color;
        }
    }
}

fn draw_sprite(buf: &mut [egui::Color32], map: SpriteMap, x: i32, y: i32, colors: &CrewColors) {
    for (r, row) in map.iter().enumerate() {
        for (col, &b) in row.as_bytes().iter().enumerate() {
            if let Some(c) = sprite_color(b, colors) {
                let (px, py) = (x + col as i32, y + r as i32);
                if px >= 0 && py >= 0 && (px as usize) < NATIVE_W && (py as usize) < NATIVE_H {
                    buf[py as usize * NATIVE_W + px as usize] = c32(c);
                }
            }
        }
    }
}

// AMBER/RED/CYAN are theme-invariant status colors, single-sourced from
// `sprites` (used by console screens/LEDs too). DARK_RED is a local-only
// flicker accent for the Breached screen, not shared elsewhere.
const DARK_RED: Rgb = Rgb(0x5e, 0x1f, 0x26);

const WALL_H: i32 = 120;
const FLOOR_CORNER: i32 = 12;

/// Deterministic starfield seed: index math only, no `rand`. Three parallax
/// layers (0 = dim/slow, 1 = mid, 2 = bright/fast), positions wrapped into
/// the viewscreen's own coordinate space.
const fn star_seed(i: i32) -> (i32, i32, u8) {
    let x = (i * 41 + 7) % VIEWSCREEN.w;
    let y = (i * 17 + 3) % VIEWSCREEN.h;
    let layer = (i % 3) as u8;
    (x, y, layer)
}
const STAR_COUNT: i32 = 46;

fn floor_corner_cut(x: i32, y: i32, y0: i32, r: i32) -> bool {
    let y1 = NATIVE_H as i32 - 1;
    let x1 = NATIVE_W as i32 - 1;
    (y - y0 < r && x < r && x + (y - y0) < r)
        || (y - y0 < r && x1 - x < r && (x1 - x) + (y - y0) < r)
        || (y1 - y < r && x < r && x + (y1 - y) < r)
        || (y1 - y < r && x1 - x < r && (x1 - x) + (y1 - y) < r)
}

fn paint_walls(buf: &mut [egui::Color32], pal: &ScenePalette) {
    fill_rect(buf, 0, 0, NATIVE_W as i32, WALL_H, c32(pal.wall));
    // Accent light strips: a lit trim band, then evenly spaced lit columns.
    fill_rect(buf, 0, WALL_H - 6, NATIVE_W as i32, 3, c32(pal.wall_trim));
    let mut x = 10;
    while x < NATIVE_W as i32 {
        fill_rect(buf, x, 6, 4, WALL_H - 16, c32(pal.wall_lit));
        x += 60;
    }
}

fn paint_floor(buf: &mut [egui::Color32], pal: &ScenePalette) {
    for y in WALL_H..NATIVE_H as i32 {
        for x in 0..NATIVE_W as i32 {
            if floor_corner_cut(x, y, WALL_H, FLOOR_CORNER) {
                continue;
            }
            let dithered = (x / 2 + y / 2) % 2 == 0;
            let color = if dithered { pal.floor_a } else { pal.floor_b };
            buf[y as usize * NATIVE_W + x as usize] = c32(color);
        }
    }
}

fn paint_platform(buf: &mut [egui::Color32], pal: &ScenePalette) {
    let plat_x = CHAIR.x - 46;
    let plat_y = CHAIR.y - 6;
    let plat_w = CHAIR.w + 92;
    let plat_h = CHAIR.h + 40;
    fill_rect(buf, plat_x, plat_y, plat_w, plat_h, c32(pal.platform));
    fill_rect(buf, plat_x, plat_y, plat_w, 3, c32(pal.platform_edge));
}

fn paint_viewscreen(
    buf: &mut [egui::Color32],
    scene: &SceneState,
    pal: &ScenePalette,
    now: f64,
    reduce_motion: bool,
) {
    let b = VIEWSCREEN;
    fill_rect(buf, b.x - 3, b.y - 3, b.w + 6, 3, c32(pal.hull));
    fill_rect(buf, b.x - 3, b.y + b.h, b.w + 6, 3, c32(pal.hull));
    fill_rect(buf, b.x - 3, b.y, 3, b.h, c32(pal.hull));
    fill_rect(buf, b.x + b.w, b.y, 3, b.h, c32(pal.hull));
    fill_rect(buf, b.x, b.y, b.w, b.h, c32(pal.viewscreen_bg));

    for i in 0..STAR_COUNT {
        let (sx, sy, layer) = star_seed(i);
        let speed = match layer {
            0 => 3.0,
            1 => 6.0,
            _ => 12.0,
        };
        let x = if reduce_motion {
            sx as f32
        } else {
            (sx as f32 - now as f32 * speed).rem_euclid(b.w as f32)
        };
        let base = match layer {
            0 => pal.star_dim,
            1 => pal.star_mid,
            _ => pal.star_bright,
        };
        let color = if reduce_motion {
            base
        } else {
            let twinkle = ((now * 3.0 + f64::from(i) * 0.7).sin() * 0.5 + 0.5) > 0.7;
            if twinkle { pal.star_bright } else { base }
        };
        fill_rect(buf, b.x + x as i32, b.y + sy, 1, 1, c32(color));
    }

    if scene.overflow > 0 {
        let n = scene.overflow.min(20) as i32;
        let chip_w = 6;
        let gap = 2;
        let total_w = n * chip_w + (n - 1) * gap;
        let start_x = b.x + b.w / 2 - total_w / 2;
        let y = b.y + b.h + 6;
        for k in 0..n {
            fill_rect(buf, start_x + k * (chip_w + gap), y, chip_w, 4, c32(AMBER));
        }
    }
}

fn paint_side_console(
    buf: &mut [egui::Color32],
    b: crate::deck::scene::Box2,
    pal: &ScenePalette,
    now: f64,
    reduce_motion: bool,
    phase: f64,
) {
    fill_rect(buf, b.x, b.y, b.w, b.h, c32(pal.console_body));
    fill_rect(
        buf,
        b.x + 2,
        b.y + 2,
        b.w - 4,
        b.h - 4,
        c32(pal.console_edge),
    );
    let tiles = 4;
    let tile_w = (b.w - 8) / tiles;
    for i in 0..tiles {
        let on = reduce_motion || (now * 2.0 + phase + f64::from(i) * 0.5) as i64 % 2 == 0;
        let color = if on { CYAN } else { pal.console_edge };
        fill_rect(
            buf,
            b.x + 4 + i * tile_w,
            b.y + 4,
            tile_w - 2,
            b.h - 8,
            c32(color),
        );
    }
}

fn paint_helm_consoles(
    buf: &mut [egui::Color32],
    scene: &SceneState,
    pal: &ScenePalette,
    now: f64,
    reduce_motion: bool,
) {
    for (i, slot) in scene.consoles.iter().enumerate().take(MAX_CONSOLES) {
        let b = console_box(i);
        fill_rect(buf, b.x, b.y, b.w, b.h, c32(pal.console_body));

        let sx = b.x + 3;
        let sy = b.y + 3;
        let sw = b.w - 6;
        let sh = b.h - 10;
        fill_rect(buf, sx, sy, sw, sh, c32(pal.screen_idle));

        let (led, screen) = visual_colors(slot.visual);
        if let Some(mut sc) = screen {
            if slot.visual == ConsoleVisual::Breached
                && !reduce_motion
                && (now * 4.0) as i64 % 2 == 0
            {
                sc = DARK_RED;
            }
            if slot.visual == ConsoleVisual::Working {
                // Working must read as visually distinct from RebaseWarn
                // (which stays a solid fill): dim the body toward the
                // console housing, then draw the scroll lines in the
                // bright status color so they're actually visible against
                // it, instead of both being the same solid amber.
                let dim = blend(sc, pal.console_body, 0.65);
                fill_rect(buf, sx, sy, sw, sh, c32(dim));
                let offset = if reduce_motion { 0.0 } else { now * 10.0 };
                for k in 0..2_i32 {
                    let ly = sy
                        + ((offset as i64 + i as i64 + i64::from(k * 4)).rem_euclid(sh as i64))
                            as i32;
                    fill_rect(buf, sx + 1, ly, sw - 2, 1, c32(sc));
                }
            } else {
                fill_rect(buf, sx, sy, sw, sh, c32(sc));
            }
        }
        fill_rect(buf, b.x + b.w - 6, b.y + b.h - 5, 3, 3, c32(led));
    }
}

fn paint_tactical(
    buf: &mut [egui::Color32],
    scene: &SceneState,
    pal: &ScenePalette,
    now: f64,
    reduce_motion: bool,
) {
    let b = TACTICAL;
    fill_rect(buf, b.x, b.y, b.w, b.h, c32(pal.console_body));
    fill_rect(
        buf,
        b.x + 2,
        b.y + 2,
        b.w - 4,
        b.h - 4,
        c32(pal.console_edge),
    );
    if now < scene.tactical_blink_until && (reduce_motion || (now * 8.0) as i64 % 2 == 0) {
        fill_rect(buf, b.x + 2, b.y + 2, b.w - 4, b.h - 4, c32(RED));
    }
}

fn paint_turbolift(buf: &mut [egui::Color32], scene: &SceneState, pal: &ScenePalette) {
    let b = LIFT;
    let cx = (b.x + b.w / 2) as f32;
    let cy = (b.y + b.h / 2) as f32;
    let near = scene.agents.iter().any(|a| {
        let (dx, dy) = (a.x - cx, a.y - cy);
        (dx * dx + dy * dy).sqrt() <= 30.0
    });
    fill_rect(buf, b.x, b.y, b.w, b.h, c32(pal.lift_frame));
    if near {
        fill_rect(buf, b.x + 2, b.y + 2, 4, b.h - 4, c32(pal.lift_door));
        fill_rect(buf, b.x + b.w - 6, b.y + 2, 4, b.h - 4, c32(pal.lift_door));
        fill_rect(
            buf,
            b.x + 6,
            b.y + 2,
            b.w - 12,
            b.h - 4,
            c32(pal.lift_door_dark),
        );
    } else {
        fill_rect(buf, b.x + 2, b.y + 2, b.w - 4, b.h - 4, c32(pal.lift_door));
        fill_rect(
            buf,
            b.x + b.w / 2 - 1,
            b.y + 2,
            2,
            b.h - 4,
            c32(pal.lift_door_dark),
        );
    }
}

fn paint_kobayashi(
    buf: &mut [egui::Color32],
    scene: &SceneState,
    pal: &ScenePalette,
    now: f64,
    reduce_motion: bool,
) {
    let b = KOBA;
    fill_rect(buf, b.x, b.y, b.w, b.h, c32(pal.koba_frame));
    let mut hx = b.x;
    let mut stripe = 0;
    while hx < b.x + b.w {
        let color = if stripe % 2 == 0 {
            pal.hazard_a
        } else {
            pal.hazard_b
        };
        fill_rect(buf, hx, b.y, 4, 3, c32(color));
        fill_rect(buf, hx, b.y + b.h - 3, 4, 3, c32(color));
        hx += 4;
        stripe += 1;
    }
    fill_rect(buf, b.x + 3, b.y + 3, b.w - 6, b.h - 6, c32(pal.koba_door));

    if scene.koba_active {
        let glow = if reduce_motion {
            blend(pal.koba_door, RED, 0.5)
        } else if (now * 3.0) as i64 % 2 == 0 {
            RED
        } else {
            pal.koba_door
        };
        fill_rect(buf, b.x + b.w / 2 - 4, b.y + b.h / 2 - 6, 8, 10, c32(glow));
    }
}

fn paint_chair(buf: &mut [egui::Color32]) {
    let colors = crew_colors(CrewStation::Command, 0);
    let x = CHAIR.x - (CHAIR_CAPT[0].len() as i32 - CHAIR.w) / 2;
    let y = CHAIR.y + CHAIR.h - CHAIR_CAPT.len() as i32;
    draw_sprite(buf, CHAIR_CAPT, x, y, &colors);
}

fn paint_agents(buf: &mut [egui::Color32], scene: &SceneState, now: f64) {
    for a in &scene.agents {
        let colors = crew_colors(CrewStation::Helm, a.hair);
        let map = match a.phase {
            AgentPhase::Walking | AgentPhase::Leaving => {
                if (now * 6.0) as i64 % 2 == 0 {
                    CREW_WALK_A
                } else {
                    CREW_WALK_B
                }
            }
            _ => CREW_BACK,
        };
        let sprite_h = map.len() as i32;
        let x = a.x as i32;
        let y = a.y as i32 - sprite_h;
        draw_sprite(buf, map, x, y, &colors);
        if now < a.alarm_until {
            draw_sprite(buf, EXCLAIM, x + 5, y - 6, &colors);
        }
    }
}

/// Pure buffer painting in painter's order: no egui context involved, so
/// this is directly testable.
pub fn paint_scene(
    buf: &mut [egui::Color32],
    scene: &SceneState,
    pal: &ScenePalette,
    now: f64,
    reduce_motion: bool,
) {
    paint_walls(buf, pal);
    paint_floor(buf, pal);
    paint_platform(buf, pal);
    paint_viewscreen(buf, scene, pal, now, reduce_motion);
    paint_side_console(buf, COMPUTER, pal, now, reduce_motion, 0.0);
    paint_side_console(buf, COMMS, pal, now, reduce_motion, 1.3);
    paint_helm_consoles(buf, scene, pal, now, reduce_motion);
    paint_tactical(buf, scene, pal, now, reduce_motion);
    paint_turbolift(buf, scene, pal);
    paint_kobayashi(buf, scene, pal, now, reduce_motion);
    paint_chair(buf);
    paint_agents(buf, scene, now);
}

pub fn draw_deck(
    canvas: &mut DeckCanvas,
    ui: &mut egui::Ui,
    app: &AppState,
    palette: DeckPalette,
    reduce_motion: bool,
) -> Option<DeckAction> {
    let now = ui.input(|i| i.time);
    canvas.scene.sync(app, now, reduce_motion);
    let pal = scene_palette(palette);
    paint_scene(&mut canvas.buf, &canvas.scene, pal, now, reduce_motion);

    let image = egui::ColorImage::new([NATIVE_W, NATIVE_H], canvas.buf.clone());
    match &mut canvas.texture {
        Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
        None => {
            canvas.texture = Some(ui.ctx().load_texture(
                "deck",
                image,
                egui::TextureOptions::NEAREST,
            ));
        }
    }
    let tex = canvas.texture.as_ref().expect("texture just set");

    let avail = ui.available_rect_before_wrap();
    let scale = integer_scale(avail.width(), avail.height()) as f32;
    let size = egui::vec2(NATIVE_W as f32 * scale, NATIVE_H as f32 * scale);
    let rect = egui::Rect::from_center_size(avail.center(), size);
    // Snap to integer screen coords: a half-pixel offset here would smear
    // the NEAREST-filtered blit across texel boundaries.
    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());
    canvas.last_rect = Some(rect);
    let response = ui.allocate_rect(rect, egui::Sense::click());
    ui.painter().image(
        tex.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    // Pointer -> native coords.
    let hit = response.hover_pos().and_then(|p| {
        let nx = (p.x - rect.min.x) / scale;
        let ny = (p.y - rect.min.y) / scale;
        input::hit_test(&canvas.scene, nx, ny)
    });
    if let Some(HitInfo { name, hint, action }) = &hit {
        response.clone().on_hover_ui(|ui| {
            ui.strong(*name);
            ui.label(hint);
        });
        if action.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    if canvas.scene.any_motion() && !reduce_motion {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    } else {
        // Ambient starfield drift continues on an idle/paused/complete
        // bridge too - not just while a mission is live - so this isn't
        // gated on `mission_live`.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }

    if response.clicked() {
        return hit.and_then(|h| h.action);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::scene::{NATIVE_H, NATIVE_W, SceneState, console_box};
    use crate::deck::sprites::scene_palette;
    use crate::state::AppState;
    use bridge_core::{BridgeEvent, DeckPalette, WorkstreamId, WorkstreamStatus};

    #[test]
    fn paint_fills_native_buffer_without_panicking() {
        let mut buf = vec![egui::Color32::BLACK; NATIVE_W * NATIVE_H];
        let mut app = AppState::default();
        for status in [
            WorkstreamStatus::Working,
            WorkstreamStatus::Breached { round: 1 },
        ] {
            app.apply(BridgeEvent::WorkstreamStatus {
                id: WorkstreamId::new(),
                status,
            });
        }
        let mut scene = SceneState::default();
        scene.sync(&app, 1.0, true);
        paint_scene(
            &mut buf,
            &scene,
            scene_palette(DeckPalette::Federation),
            1.0,
            true,
        );
        // Wall color reached the top-left corner.
        assert_ne!(buf[0], egui::Color32::BLACK);
    }

    #[test]
    fn working_console_led_is_amber() {
        let mut buf = vec![egui::Color32::BLACK; NATIVE_W * NATIVE_H];
        let mut app = AppState::default();
        app.apply(BridgeEvent::WorkstreamStatus {
            id: WorkstreamId::new(),
            status: WorkstreamStatus::Working,
        });
        let mut scene = SceneState::default();
        scene.sync(&app, 1.0, true);
        paint_scene(
            &mut buf,
            &scene,
            scene_palette(DeckPalette::Federation),
            1.0,
            true,
        );
        let b = console_box(0);
        let (lx, ly) = ((b.x + b.w - 5) as usize, (b.y + b.h - 4) as usize);
        assert_eq!(
            buf[ly * NATIVE_W + lx],
            egui::Color32::from_rgb(0xff, 0xb6, 0x48)
        );
    }

    #[test]
    fn working_console_dims_body_but_keeps_scroll_lines_bright() {
        // reduce_motion pins the scroll offset at 0.0, so with i = 0 (first
        // console) the two scroll lines land at rows sy+0 and sy+4 (the
        // `(offset + i + k*4).rem_euclid(sh)` math at k = 0, 1). sh = 8 for
        // console 0, so sy+2 is a deterministic body-only row: not a
        // scroll line, and not wrapped-around into one either.
        let mut buf = vec![egui::Color32::BLACK; NATIVE_W * NATIVE_H];
        let mut app = AppState::default();
        app.apply(BridgeEvent::WorkstreamStatus {
            id: WorkstreamId::new(),
            status: WorkstreamStatus::Working,
        });
        let mut scene = SceneState::default();
        let now = 1.0;
        scene.sync(&app, now, true);
        paint_scene(
            &mut buf,
            &scene,
            scene_palette(DeckPalette::Federation),
            now,
            true,
        );
        let b = console_box(0);
        let sx = (b.x + 3) as usize;
        let sy = b.y + 3;
        let bright = egui::Color32::from_rgb(0xff, 0xb6, 0x48); // AMBER

        let scroll_line = buf[sy as usize * NATIVE_W + sx + 1];
        assert_eq!(
            scroll_line, bright,
            "scroll-line pixel must stay in the bright status color"
        );

        let body = buf[(sy + 2) as usize * NATIVE_W + sx + 1];
        assert_ne!(
            body, bright,
            "console body must be dimmed, not solid bright, behind the scroll lines"
        );
    }

    #[test]
    fn integer_scale_picks_floor_min_dimension() {
        assert_eq!(integer_scale(960.0, 620.0), 2);
        assert_eq!(integer_scale(1440.0, 930.0), 3);
        assert_eq!(integer_scale(500.0, 200.0), 1); // never 0
    }
}
