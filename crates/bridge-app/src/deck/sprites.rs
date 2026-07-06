//! Sprite string maps, palettes, and status color tables. Data only:
//! one string per row, one byte per pixel, keyed into palette structs.

use bridge_core::DeckPalette;
use bridge_core::WorkstreamStatus;

pub type SpriteMap = &'static [&'static str];

pub const CREW_BACK: SpriteMap = &[
    "....HHHH....",
    "...HHHHHH...",
    "...HHHHHH...",
    "...HHHHHH...",
    "....KKKK....",
    "...UUUUUU...",
    "..UUUUUUUU..",
    ".UUUUUUUUUU.",
    ".KUUUUUUUUK.",
    "...UUUUUU...",
    "...DDDDDD...",
    "...PP..PP...",
    "...PP..PP...",
    "...BB..BB...",
];

pub const CREW_WALK_A: SpriteMap = &[
    "....HHHH....",
    "...HHHHHH...",
    "...HHHHHH...",
    "...HHHHHH...",
    "....KKKK....",
    "...UUUUUU...",
    "..UUUUUUUU..",
    ".UUUUUUUUUU.",
    ".KUUUUUUUUK.",
    "...UUUUUU...",
    "...DDDDDD...",
    "..PP....PP..",
    "..PP....PP..",
    "..BB....BB..",
];

pub const CREW_WALK_B: SpriteMap = &[
    "....HHHH....",
    "...HHHHHH...",
    "...HHHHHH...",
    "...HHHHHH...",
    "....KKKK....",
    "...UUUUUU...",
    "..UUUUUUUU..",
    ".UUUUUUUUUU.",
    ".KUUUUUUUUK.",
    "...UUUUUU...",
    "...DDDDDD...",
    "....PPPP....",
    "....PP.PP...",
    "....BB.BB...",
];

/// Captain's chair from behind, captain's head above the backrest.
pub const CHAIR_CAPT: SpriteMap = &[
    "......HHHH......",
    ".....HHHHHH.....",
    ".....HHHHHH.....",
    "....cCCCCCCc....",
    "...cCCCCCCCCc...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...cCCCCCCCCc...",
    "....cCCCCCCc....",
    "..ccc..cc..ccc..",
    "..cc........cc..",
];

pub const EXCLAIM: SpriteMap = &["RR", "RR", "RR", "..", "RR"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug)]
pub struct ScenePalette {
    pub wall: Rgb,
    pub wall_lit: Rgb,
    pub wall_trim: Rgb,
    pub floor_a: Rgb,
    pub floor_b: Rgb,
    pub platform: Rgb,
    pub platform_edge: Rgb,
    pub hull: Rgb,
    pub console_body: Rgb,
    pub console_edge: Rgb,
    pub screen_idle: Rgb,
    pub viewscreen_bg: Rgb,
    pub star_dim: Rgb,
    pub star_mid: Rgb,
    pub star_bright: Rgb,
    pub lift_door: Rgb,
    pub lift_door_dark: Rgb,
    pub lift_frame: Rgb,
    pub koba_door: Rgb,
    pub koba_frame: Rgb,
    pub hazard_a: Rgb,
    pub hazard_b: Rgb,
}

static FEDERATION: ScenePalette = ScenePalette {
    wall: Rgb(0x2b, 0x2f, 0x45),
    wall_lit: Rgb(0x3a, 0x40, 0x5c),
    wall_trim: Rgb(0x4c, 0x53, 0x78),
    floor_a: Rgb(0x6e, 0x4a, 0x52),
    floor_b: Rgb(0x65, 0x43, 0x49),
    platform: Rgb(0x7c, 0x54, 0x5c),
    platform_edge: Rgb(0x4a, 0x32, 0x38),
    hull: Rgb(0xb3, 0xa5, 0x8c),
    console_body: Rgb(0x1c, 0x22, 0x33),
    console_edge: Rgb(0x39, 0x41, 0x5e),
    screen_idle: Rgb(0x23, 0x2c, 0x47),
    viewscreen_bg: Rgb(0x05, 0x09, 0x14),
    star_dim: Rgb(0x4a, 0x6a, 0x8a),
    star_mid: Rgb(0x8f, 0xb7, 0xd9),
    star_bright: Rgb(0xe8, 0xf2, 0xfb),
    lift_door: Rgb(0x8c, 0x80, 0x6c),
    lift_door_dark: Rgb(0x6e, 0x64, 0x54),
    lift_frame: Rgb(0xb3, 0xa5, 0x8c),
    koba_door: Rgb(0x1a, 0x14, 0x20),
    koba_frame: Rgb(0x3a, 0x2a, 0x30),
    hazard_a: Rgb(0xd9, 0xa4, 0x41),
    hazard_b: Rgb(0x23, 0x1a, 0x10),
};

static DARK_OPS: ScenePalette = ScenePalette {
    wall: Rgb(0x13, 0x1a, 0x2e),
    wall_lit: Rgb(0x1b, 0x24, 0x40),
    wall_trim: Rgb(0x2a, 0x35, 0x57),
    floor_a: Rgb(0x1b, 0x22, 0x33),
    floor_b: Rgb(0x17, 0x1d, 0x2c),
    platform: Rgb(0x23, 0x2c, 0x47),
    platform_edge: Rgb(0x11, 0x16, 0x24),
    hull: Rgb(0x3d, 0x4a, 0x70),
    console_body: Rgb(0x10, 0x16, 0x2a),
    console_edge: Rgb(0x2f, 0x3b, 0x61),
    screen_idle: Rgb(0x1a, 0x23, 0x40),
    viewscreen_bg: Rgb(0x04, 0x07, 0x0f),
    star_dim: Rgb(0x2a, 0x5a, 0x7a),
    star_mid: Rgb(0x59, 0xc8, 0xff),
    star_bright: Rgb(0xd8, 0xf1, 0xff),
    lift_door: Rgb(0x2a, 0x33, 0x50),
    lift_door_dark: Rgb(0x1d, 0x25, 0x40),
    lift_frame: Rgb(0x3d, 0x4a, 0x70),
    koba_door: Rgb(0x14, 0x0e, 0x1a),
    koba_frame: Rgb(0x2a, 0x1f, 0x2a),
    hazard_a: Rgb(0xc9, 0x90, 0x3a),
    hazard_b: Rgb(0x1a, 0x12, 0x08),
};

pub fn scene_palette(p: DeckPalette) -> &'static ScenePalette {
    match p {
        DeckPalette::Federation => &FEDERATION,
        DeckPalette::DarkOps => &DARK_OPS,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrewStation {
    Command,
    Helm,
    // Reserved for when the scene assigns an agent to the tactical console
    // specifically (today every walking agent renders as `Helm`); the color
    // match arm below already handles it identically to `Helm`.
    #[allow(dead_code)]
    Tactical,
}

#[derive(Debug, Clone, Copy)]
pub struct CrewColors {
    pub hair: Rgb,
    pub skin: Rgb,
    pub uniform: Rgb,
    pub uniform_dark: Rgb,
    pub pants: Rgb,
    pub boots: Rgb,
    pub chair: Rgb,
    pub chair_dark: Rgb,
    pub alert: Rgb,
}

const HAIRS: [Rgb; 5] = [
    Rgb(0x3a, 0x2a, 0x1a),
    Rgb(0x5a, 0x46, 0x32),
    Rgb(0x1a, 0x1a, 0x22),
    Rgb(0x8c, 0x6a, 0x3a),
    Rgb(0x6e, 0x3a, 0x2a),
];

pub fn crew_colors(station: CrewStation, hair: usize) -> CrewColors {
    let (uniform, uniform_dark) = match station {
        CrewStation::Command => (Rgb(0x8c, 0x2f, 0x39), Rgb(0x5e, 0x1f, 0x26)),
        CrewStation::Helm | CrewStation::Tactical => (Rgb(0xc9, 0x90, 0x3a), Rgb(0x8a, 0x5f, 0x1e)),
    };
    CrewColors {
        hair: HAIRS[hair % HAIRS.len()],
        skin: Rgb(0xe0, 0xb4, 0x8c),
        uniform,
        uniform_dark,
        pants: Rgb(0x23, 0x28, 0x3a),
        boots: Rgb(0x14, 0x16, 0x1f),
        chair: Rgb(0x3a, 0x42, 0x62),
        chair_dark: Rgb(0x26, 0x2c, 0x44),
        alert: Rgb(0xff, 0x5a, 0x4e),
    }
}

pub fn sprite_color(map_char: u8, c: &CrewColors) -> Option<Rgb> {
    match map_char {
        b'H' => Some(c.hair),
        b'K' => Some(c.skin),
        b'U' => Some(c.uniform),
        b'D' => Some(c.uniform_dark),
        b'P' => Some(c.pants),
        b'B' => Some(c.boots),
        b'C' => Some(c.chair),
        b'c' => Some(c.chair_dark),
        b'R' => Some(c.alert),
        _ => None,
    }
}

/// What a helm console screen shows for a workstream state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleVisual {
    Unassigned,
    Dim,
    Working,
    UnderTest,
    Breached,
    ReadyGreen,
    RebaseWarn,
    Merged,
    /// Failed or Flagged: steady red, no flash (needs the user, not urgency).
    Stuck,
}

pub fn console_visual(status: &WorkstreamStatus) -> ConsoleVisual {
    use WorkstreamStatus as S;
    match status {
        S::Pending => ConsoleVisual::Dim,
        S::Working => ConsoleVisual::Working,
        S::UnderTest { .. } => ConsoleVisual::UnderTest,
        S::Breached { .. } => ConsoleVisual::Breached,
        S::ReadyToMerge | S::InMergeQueue => ConsoleVisual::ReadyGreen,
        S::Rebasing | S::ConflictFix => ConsoleVisual::RebaseWarn,
        S::Merged => ConsoleVisual::Merged,
        S::Failed { .. } | S::Flagged => ConsoleVisual::Stuck,
    }
}

// Theme-invariant status colors: single-sourced here so `render.rs` (console
// screen fills, overflow chips, tactical blink, side-console tiles) doesn't
// keep its own copies.
pub(crate) const AMBER: Rgb = Rgb(0xff, 0xb6, 0x48);
pub(crate) const RED: Rgb = Rgb(0xff, 0x5a, 0x4e);
pub(crate) const GREEN: Rgb = Rgb(0x3f, 0xd6, 0x8c);
pub(crate) const CYAN: Rgb = Rgb(0x59, 0xc8, 0xff);
/// Dim/off LED and unlit-screen color.
pub(crate) const OFF: Rgb = Rgb(0x3a, 0x42, 0x62);

/// (LED color, screen color if lit).
pub fn visual_colors(v: ConsoleVisual) -> (Rgb, Option<Rgb>) {
    match v {
        ConsoleVisual::Unassigned | ConsoleVisual::Dim => (OFF, None),
        ConsoleVisual::Working => (AMBER, Some(AMBER)),
        ConsoleVisual::UnderTest => (CYAN, Some(CYAN)),
        ConsoleVisual::Breached => (RED, Some(RED)),
        ConsoleVisual::ReadyGreen | ConsoleVisual::Merged => (GREEN, Some(GREEN)),
        ConsoleVisual::RebaseWarn => (AMBER, Some(AMBER)),
        ConsoleVisual::Stuck => (RED, Some(RED)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::WorkstreamStatus;

    fn assert_rectangular(map: SpriteMap, w: usize, h: usize) {
        assert_eq!(map.len(), h);
        for row in map {
            assert_eq!(row.len(), w, "row {row:?} width");
        }
    }

    #[test]
    fn sprite_maps_are_rectangular() {
        assert_rectangular(CREW_BACK, 12, 14);
        assert_rectangular(CREW_WALK_A, 12, 14);
        assert_rectangular(CREW_WALK_B, 12, 14);
        assert_rectangular(CHAIR_CAPT, 16, 14);
        assert_rectangular(EXCLAIM, 2, 5);
    }

    #[test]
    fn every_sprite_char_resolves() {
        let c = crew_colors(CrewStation::Helm, 1);
        for map in [CREW_BACK, CREW_WALK_A, CREW_WALK_B, CHAIR_CAPT, EXCLAIM] {
            for row in map {
                for &b in row.as_bytes() {
                    if b != b'.' {
                        assert!(sprite_color(b, &c).is_some(), "unmapped char {}", b as char);
                    }
                }
            }
        }
    }

    #[test]
    fn console_visual_is_total_over_status() {
        use WorkstreamStatus as S;
        let all = [
            (S::Pending, ConsoleVisual::Dim),
            (S::Working, ConsoleVisual::Working),
            (S::UnderTest { round: 1 }, ConsoleVisual::UnderTest),
            (S::Breached { round: 1 }, ConsoleVisual::Breached),
            (S::ReadyToMerge, ConsoleVisual::ReadyGreen),
            (S::Rebasing, ConsoleVisual::RebaseWarn),
            (S::ConflictFix, ConsoleVisual::RebaseWarn),
            (S::InMergeQueue, ConsoleVisual::ReadyGreen),
            (S::Merged, ConsoleVisual::Merged),
            (S::Failed { reason: "x".into() }, ConsoleVisual::Stuck),
            (S::Flagged, ConsoleVisual::Stuck),
        ];
        for (status, expect) in all {
            assert_eq!(console_visual(&status), expect, "{status:?}");
        }
    }

    #[test]
    fn palettes_differ() {
        use bridge_core::DeckPalette;
        let fed = scene_palette(DeckPalette::Federation);
        let ops = scene_palette(DeckPalette::DarkOps);
        assert_ne!(fed.floor_a.0, ops.floor_a.0);
    }
}
