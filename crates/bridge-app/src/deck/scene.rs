//! Pure deck scene state: layout constants, console assignment, and (Task 4)
//! edge-triggered transient animation. No egui; the caller supplies time.

use crate::deck::sprites::{ConsoleVisual, console_visual};
use crate::state::AppState;
use bridge_core::{MissionState, WorkstreamId, WorkstreamStatus};
use std::collections::HashSet;

pub const NATIVE_W: usize = 480;
pub const NATIVE_H: usize = 310;
pub const MAX_CONSOLES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Box2 {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Box2 {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x as f32
            && px <= (self.x + self.w) as f32
            && py >= self.y as f32
            && py <= (self.y + self.h) as f32
    }
}

pub const VIEWSCREEN: Box2 = Box2 {
    x: 165,
    y: 16,
    w: 150,
    h: 58,
};
pub const COMPUTER: Box2 = Box2 {
    x: 42,
    y: 52,
    w: 78,
    h: 26,
};
pub const COMMS: Box2 = Box2 {
    x: 360,
    y: 52,
    w: 78,
    h: 26,
};
pub const TACTICAL: Box2 = Box2 {
    x: 140,
    y: 236,
    w: 200,
    h: 14,
};
pub const LIFT: Box2 = Box2 {
    x: 34,
    y: 232,
    w: 30,
    h: 40,
};
pub const KOBA: Box2 = Box2 {
    x: 412,
    y: 232,
    w: 34,
    h: 40,
};
pub const CHAIR: Box2 = Box2 {
    x: 232,
    y: 170,
    w: 16,
    h: 22,
};

pub const CONSOLE_W: i32 = 30;
pub const CONSOLE_H: i32 = 18;
/// Corridor row agents walk along between the lift and their console.
pub const WALK_Y: f32 = 208.0;
pub const WALK_SPEED: f32 = 34.0; // native px per second

/// Front-row console arc: 8 slots, gently curved.
pub fn console_box(i: usize) -> Box2 {
    const DY: [i32; MAX_CONSOLES] = [10, 5, 2, 0, 0, 2, 5, 10];
    Box2 {
        x: 34 + i as i32 * 54,
        y: 128 + DY[i],
        w: CONSOLE_W,
        h: CONSOLE_H,
    }
}

/// Where an agent stands when manning console `i`.
pub fn stand_point(i: usize) -> (f32, f32) {
    let b = console_box(i);
    ((b.x + b.w / 2 - 6) as f32, (b.y + b.h - 2) as f32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleSlot {
    pub ws: Option<WorkstreamId>,
    pub visual: ConsoleVisual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPhase {
    Walking,
    Seated,
    Leaving,
    Gone,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub ws: WorkstreamId,
    pub x: f32,
    pub y: f32,
    pub path: Vec<(f32, f32)>,
    pub phase: AgentPhase,
    pub alarm_until: f64,
    pub hair: usize,
}

#[derive(Debug, Default)]
pub struct SceneState {
    pub consoles: Vec<ConsoleSlot>,
    pub agents: Vec<Agent>,
    pub overflow: u32,
    pub koba_active: bool,
    pub tactical_blink_until: f64,
    pub mission_live: bool,
    seen: HashSet<WorkstreamId>,
    prev_status: std::collections::HashMap<WorkstreamId, WorkstreamStatus>,
    last_hook_len: usize,
    last_now: f64,
}

impl SceneState {
    /// Reconcile with truth. Declarative fields are recomputed every call;
    /// transients (walk-in/out, alarms, blinks) are edge-triggered by
    /// diffing against the previous call.
    pub fn sync(&mut self, app: &AppState, now: f64, reduce_motion: bool) {
        let dt = (now - self.last_now).clamp(0.0, 0.25) as f32;
        self.last_now = now;

        self.mission_live = matches!(app.mission_state, Some(MissionState::Executing));

        // Declarative: console slots from first-seen order.
        self.consoles = (0..MAX_CONSOLES)
            .map(|i| match app.workstream_order.get(i) {
                Some(id) => ConsoleSlot {
                    ws: Some(*id),
                    visual: app.workstreams[id]
                        .status
                        .as_ref()
                        .map(console_visual)
                        .unwrap_or(ConsoleVisual::Dim),
                },
                None => ConsoleSlot {
                    ws: None,
                    visual: ConsoleVisual::Unassigned,
                },
            })
            .collect();
        self.overflow = app.workstream_order.len().saturating_sub(MAX_CONSOLES) as u32;

        self.koba_active = app
            .workstreams
            .values()
            .any(|p| matches!(p.status, Some(WorkstreamStatus::UnderTest { .. })));

        // Task 4 inserts edge-triggered transients here.
        let _ = (dt, reduce_motion);
    }

    pub fn any_motion(&self) -> bool {
        self.agents.iter().any(|a| {
            matches!(a.phase, AgentPhase::Walking | AgentPhase::Leaving)
                || a.alarm_until > self.last_now
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use bridge_core::{BridgeEvent, WorkstreamId, WorkstreamStatus};

    fn app_with(statuses: &[WorkstreamStatus]) -> AppState {
        let mut app = AppState::default();
        for status in statuses {
            let id = WorkstreamId::new();
            app.apply(BridgeEvent::WorkstreamStatus {
                id,
                status: status.clone(),
            });
        }
        app
    }

    #[test]
    fn consoles_assigned_in_first_seen_order_with_status_visuals() {
        let app = app_with(&[WorkstreamStatus::Working, WorkstreamStatus::Pending]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        assert_eq!(scene.consoles.len(), MAX_CONSOLES);
        assert_eq!(scene.consoles[0].ws, Some(app.workstream_order[0]));
        assert_eq!(
            scene.consoles[0].visual,
            crate::deck::sprites::ConsoleVisual::Working
        );
        assert_eq!(
            scene.consoles[1].visual,
            crate::deck::sprites::ConsoleVisual::Dim
        );
        assert_eq!(scene.consoles[2].ws, None);
        assert_eq!(scene.overflow, 0);
    }

    #[test]
    fn ninth_workstream_overflows_into_chip() {
        let app = app_with(&vec![WorkstreamStatus::Working; 10]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        assert_eq!(scene.consoles.len(), MAX_CONSOLES);
        assert_eq!(scene.overflow, 2);
    }

    #[test]
    fn koba_active_iff_any_under_test() {
        let app = app_with(&[WorkstreamStatus::UnderTest { round: 1 }]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        assert!(scene.koba_active);
        let app2 = app_with(&[WorkstreamStatus::Working]);
        let mut scene2 = SceneState::default();
        scene2.sync(&app2, 0.0, true);
        assert!(!scene2.koba_active);
    }

    #[test]
    fn console_boxes_fit_native_and_do_not_overlap() {
        for i in 0..MAX_CONSOLES {
            let b = console_box(i);
            assert!(b.x >= 0 && b.y >= 0);
            assert!((b.x + b.w) as usize <= NATIVE_W);
            assert!((b.y + b.h) as usize <= NATIVE_H);
            for j in (i + 1)..MAX_CONSOLES {
                let o = console_box(j);
                let disjoint_x = b.x + b.w <= o.x || o.x + o.w <= b.x;
                assert!(disjoint_x, "consoles {i} and {j} overlap");
            }
        }
    }
}
