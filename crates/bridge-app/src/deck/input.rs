//! Native-coordinate hit testing. Pure; no egui.

use crate::deck::DeckAction;
use crate::deck::scene::{
    Box2, CHAIR, COMMS, COMPUTER, KOBA, LIFT, MAX_CONSOLES, SceneState, TACTICAL, VIEWSCREEN,
    console_box,
};

#[derive(Debug, Clone, PartialEq)]
pub struct HitInfo {
    pub action: Option<DeckAction>,
    pub name: &'static str,
    pub hint: String,
}

fn inflate(b: Box2, left: i32, top: i32, right: i32, bottom: i32) -> Box2 {
    Box2 {
        x: b.x - left,
        y: b.y - top,
        w: b.w + left + right,
        h: b.h + top + bottom,
    }
}

/// Topmost hit wins: consoles (with their standing agents) over furniture.
pub fn hit_test(scene: &SceneState, x: f32, y: f32) -> Option<HitInfo> {
    for i in 0..MAX_CONSOLES {
        let hot = inflate(console_box(i), 2, 16, 2, 2);
        if hot.contains(x, y) {
            let slot = &scene.consoles[i];
            return Some(match slot.ws {
                Some(ws) => HitInfo {
                    action: Some(DeckAction::SelectWorkstream(ws)),
                    name: "HELM CONSOLE",
                    hint: format!("{:?} - click for workstream detail.", slot.visual),
                },
                None => HitInfo {
                    action: None,
                    name: "HELM CONSOLE",
                    hint: "Unassigned station.".into(),
                },
            });
        }
    }
    let chair_hot = inflate(CHAIR, 6, 12, 6, 6);
    if chair_hot.contains(x, y) {
        return Some(HitInfo {
            action: Some(DeckAction::HailCaptain),
            name: "CAPTAIN",
            hint: "Hail the Captain.".into(),
        });
    }
    if inflate(TACTICAL, 0, 16, 0, 6).contains(x, y) {
        return Some(HitInfo {
            action: Some(DeckAction::OpenTactical),
            name: "TACTICAL",
            hint: "Guardrail adjudications.".into(),
        });
    }
    if inflate(KOBA, 3, 3, 3, 3).contains(x, y) {
        return Some(HitInfo {
            action: Some(DeckAction::OpenKobayashi),
            name: "KOBAYASHI MARU",
            hint: "Latest battle report.".into(),
        });
    }
    if COMPUTER.contains(x, y) {
        return Some(HitInfo {
            action: Some(DeckAction::OpenComputer),
            name: "SHIP'S COMPUTER",
            hint: "Mission archive.".into(),
        });
    }
    if COMMS.contains(x, y) {
        return Some(HitInfo {
            action: None,
            name: "COMMS",
            hint: "Mission report - compiled at mission end.".into(),
        });
    }
    if inflate(LIFT, 3, 3, 3, 3).contains(x, y) {
        return Some(HitInfo {
            action: None,
            name: "TURBOLIFT",
            hint: "Agents arrive and depart here.".into(),
        });
    }
    if VIEWSCREEN.contains(x, y) {
        return Some(HitInfo {
            action: Some(DeckAction::OpenMissionStatus),
            name: "VIEWSCREEN",
            hint: if scene.mission_live {
                "Mission underway.".into()
            } else {
                "No mission on screen.".into()
            },
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::scene::{CHAIR, KOBA, SceneState, TACTICAL, VIEWSCREEN, console_box};
    use crate::state::AppState;
    use bridge_core::{BridgeEvent, WorkstreamId, WorkstreamStatus};

    fn scene_with_one() -> (SceneState, WorkstreamId) {
        let mut app = AppState::default();
        let id = WorkstreamId::new();
        app.apply(BridgeEvent::WorkstreamStatus {
            id,
            status: WorkstreamStatus::Working,
        });
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        (scene, id)
    }

    fn center_of(b: crate::deck::scene::Box2) -> (f32, f32) {
        ((b.x + b.w / 2) as f32, (b.y + b.h / 2) as f32)
    }

    #[test]
    fn chair_hails_captain() {
        let (scene, _) = scene_with_one();
        let (x, y) = center_of(CHAIR);
        assert_eq!(
            hit_test(&scene, x, y).unwrap().action,
            Some(DeckAction::HailCaptain)
        );
    }

    #[test]
    fn manned_console_selects_workstream() {
        let (scene, id) = scene_with_one();
        let (x, y) = center_of(console_box(0));
        assert_eq!(
            hit_test(&scene, x, y).unwrap().action,
            Some(DeckAction::SelectWorkstream(id))
        );
    }

    #[test]
    fn unmanned_console_is_tooltip_only() {
        let (scene, _) = scene_with_one();
        let (x, y) = center_of(console_box(3));
        let hit = hit_test(&scene, x, y).unwrap();
        assert_eq!(hit.action, None);
    }

    #[test]
    fn stations_route_to_their_panels() {
        let (scene, _) = scene_with_one();
        let (tx, ty) = center_of(TACTICAL);
        assert_eq!(
            hit_test(&scene, tx, ty).unwrap().action,
            Some(DeckAction::OpenTactical)
        );
        let (kx, ky) = center_of(KOBA);
        assert_eq!(
            hit_test(&scene, kx, ky).unwrap().action,
            Some(DeckAction::OpenKobayashi)
        );
    }

    #[test]
    fn viewscreen_opens_mission_status() {
        let (scene, _) = scene_with_one();
        let (vx, vy) = center_of(VIEWSCREEN);
        let hit = hit_test(&scene, vx, vy).unwrap();
        assert_eq!(hit.name, "VIEWSCREEN");
        assert_eq!(hit.action, Some(DeckAction::OpenMissionStatus));
    }

    #[test]
    fn empty_floor_hits_nothing() {
        let (scene, _) = scene_with_one();
        assert!(hit_test(&scene, 240.0, 290.0).is_none());
    }
}
