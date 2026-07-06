//! The 16-bit captain's deck: pure scene state driven by AppState, pixel
//! sprites and palettes, a software-rendered framebuffer blitted through
//! one NEAREST egui texture, and native-coordinate hit-testing.

#![allow(dead_code)]
// Staged build: the deck is consumed by render/ui integration (Tasks 6-7); remove this allow then.

pub mod input;
pub mod render;
pub mod scene;
pub mod sprites;

use bridge_core::WorkstreamId;

/// What a click on the deck means. `ui.rs` translates these into
/// selection changes and interim detail windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckAction {
    HailCaptain,
    SelectWorkstream(WorkstreamId),
    OpenTactical,
    OpenKobayashi,
    OpenComputer,
}
