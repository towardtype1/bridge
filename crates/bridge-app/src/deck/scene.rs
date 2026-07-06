//! Pure deck scene state: layout constants, console assignment, and (Task 4)
//! edge-triggered transient animation. No egui; the caller supplies time.

use crate::deck::sprites::{ConsoleVisual, console_visual};
use crate::state::AppState;
use bridge_core::{DecisionKind, MissionState, WorkstreamId, WorkstreamStatus};
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
    /// Console slot lifecycle, persistent across syncs: `slots[i]` is the
    /// workstream currently manning console `i`, or `None` if empty. Unlike
    /// `consoles` (rebuilt declaratively every call from this), a slot
    /// assignment sticks until explicitly released.
    slots: Vec<Option<WorkstreamId>>,
    prev_status: std::collections::HashMap<WorkstreamId, WorkstreamStatus>,
    /// Timestamp of the newest tactical-feed record seen so far. Edge-
    /// triggers the blink on identity/recency rather than feed length,
    /// since `push_bounded` front-trims the feed at `MAX_FEED` - length
    /// alone stalls forever once the feed saturates.
    last_hook_ts: Option<chrono::DateTime<chrono::Utc>>,
    last_now: f64,
}

/// True once a workstream has reached a terminal, departed state: its
/// console slot may be reclaimed once its agent has also left the deck.
fn is_departed(app: &AppState, id: &WorkstreamId) -> bool {
    matches!(
        app.workstreams.get(id).and_then(|p| p.status.as_ref()),
        Some(WorkstreamStatus::Merged)
    )
}

impl SceneState {
    /// Reconcile with truth. Declarative fields are recomputed every call;
    /// transients (walk-in/out, alarms, blinks) are edge-triggered by
    /// diffing against the previous call.
    pub fn sync(&mut self, app: &AppState, now: f64, reduce_motion: bool) {
        let dt = (now - self.last_now).clamp(0.0, 0.25) as f32;
        self.last_now = now;

        self.mission_live = matches!(app.mission_state, Some(MissionState::Executing));

        if self.slots.len() != MAX_CONSOLES {
            self.slots = vec![None; MAX_CONSOLES];
        }

        // (a) Release a slot once its workstream has departed (Merged) and
        // its agent has actually left the deck - not the moment it merges,
        // so a departing agent's console keeps showing Merged green while
        // it's still walking out.
        for slot in &mut self.slots {
            if let Some(id) = *slot {
                let has_agent = self.agents.iter().any(|a| a.ws == id);
                if is_departed(app, &id) && !has_agent {
                    *slot = None;
                }
            }
        }

        // (b) Fill empty slots, left to right, with the earliest
        // not-yet-slotted, still-live workstreams. A slot assignment made
        // here is exactly the "ws first gets a slot" edge: it can't have
        // been in `slotted` before, so it wasn't assigned in any prior
        // sync either.
        let slotted: HashSet<WorkstreamId> = self.slots.iter().flatten().copied().collect();
        let mut candidates = app
            .workstream_order
            .iter()
            .filter(|id| !slotted.contains(id) && !is_departed(app, id));
        let mut newly_assigned: Vec<(usize, WorkstreamId)> = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none()
                && let Some(&id) = candidates.next()
            {
                *slot = Some(id);
                newly_assigned.push((i, id));
            }
        }

        // (c) Rebuild the declarative console view from slots.
        self.consoles = self
            .slots
            .iter()
            .map(|slot| match slot {
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
        let slotted_now: HashSet<WorkstreamId> = self.slots.iter().flatten().copied().collect();
        self.overflow = app
            .workstream_order
            .iter()
            .filter(|id| !slotted_now.contains(id) && !is_departed(app, id))
            .count() as u32;

        self.koba_active = app
            .workstreams
            .values()
            .any(|p| matches!(p.status, Some(WorkstreamStatus::UnderTest { .. })));

        // --- edge-triggered transients ------------------------------------
        for (i, id) in newly_assigned {
            let target = stand_point(i);
            let start = ((LIFT.x + 9) as f32, (LIFT.y + 8) as f32);
            self.agents.push(if reduce_motion {
                Agent {
                    ws: id,
                    x: target.0,
                    y: target.1,
                    path: Vec::new(),
                    phase: AgentPhase::Seated,
                    alarm_until: 0.0,
                    hair: i + 1,
                }
            } else {
                Agent {
                    ws: id,
                    x: start.0,
                    y: start.1,
                    path: vec![(start.0, WALK_Y), (target.0, WALK_Y), target],
                    phase: AgentPhase::Walking,
                    alarm_until: 0.0,
                    hair: i + 1,
                }
            });
        }

        for id in &app.workstream_order {
            let status = app.workstreams[id].status.clone();
            let prev = self.prev_status.get(id);
            let became = |m: fn(&WorkstreamStatus) -> bool| {
                status.as_ref().is_some_and(m) && !prev.is_some_and(m)
            };
            if became(|s| matches!(s, WorkstreamStatus::Merged))
                && let Some(a) = self.agents.iter_mut().find(|a| a.ws == *id)
            {
                if reduce_motion {
                    a.phase = AgentPhase::Gone;
                } else {
                    a.phase = AgentPhase::Leaving;
                    a.path = vec![
                        (a.x, WALK_Y),
                        ((LIFT.x + 9) as f32, WALK_Y),
                        ((LIFT.x + 9) as f32, (LIFT.y + 8) as f32),
                    ];
                }
            }
            if became(|s| matches!(s, WorkstreamStatus::Breached { .. }))
                && let Some(a) = self.agents.iter_mut().find(|a| a.ws == *id)
            {
                a.alarm_until = now + 3.0;
            }
            if let Some(s) = status {
                self.prev_status.insert(*id, s);
            }
        }

        if let Some(newest) = app.tactical_feed.last() {
            let is_new = self.last_hook_ts.is_none_or(|ts| newest.timestamp > ts);
            if is_new {
                if newest.decision != DecisionKind::Allow {
                    self.tactical_blink_until = now + 1.5;
                }
                self.last_hook_ts = Some(newest.timestamp);
            }
        }

        self.advance_agents(dt);
        self.agents.retain(|a| a.phase != AgentPhase::Gone);
    }

    pub fn any_motion(&self) -> bool {
        self.agents.iter().any(|a| {
            matches!(a.phase, AgentPhase::Walking | AgentPhase::Leaving)
                || a.alarm_until > self.last_now
        })
    }

    fn advance_agents(&mut self, dt: f32) {
        for a in &mut self.agents {
            if !matches!(a.phase, AgentPhase::Walking | AgentPhase::Leaving) {
                continue;
            }
            let mut budget = WALK_SPEED * dt;
            while budget > 0.0 {
                let Some(&(tx, ty)) = a.path.first() else {
                    a.phase = match a.phase {
                        AgentPhase::Leaving => AgentPhase::Gone,
                        _ => AgentPhase::Seated,
                    };
                    break;
                };
                let (dx, dy) = (tx - a.x, ty - a.y);
                let dist = (dx * dx + dy * dy).sqrt();
                if dist <= budget {
                    a.x = tx;
                    a.y = ty;
                    a.path.remove(0);
                    budget -= dist;
                } else {
                    a.x += dx / dist * budget;
                    a.y += dy / dist * budget;
                    budget = 0.0;
                }
            }
        }
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
    fn merging_early_workstream_frees_its_slot_for_the_ninth() {
        let mut app = app_with(&vec![WorkstreamStatus::Working; 9]);
        let ids = app.workstream_order.clone();
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true); // reduce_motion: agents seated instantly

        // First 8 slotted in order; the 9th overflows with no console/agent.
        assert_eq!(scene.consoles.len(), MAX_CONSOLES);
        for (i, id) in ids.iter().take(MAX_CONSOLES).enumerate() {
            assert_eq!(scene.consoles[i].ws, Some(*id));
        }
        assert_eq!(scene.overflow, 1);
        assert_eq!(scene.agents.len(), MAX_CONSOLES);
        assert!(!scene.agents.iter().any(|a| a.ws == ids[8]));

        // Merge the first workstream. Its slot isn't reclaimed until its
        // agent has actually departed (this sync sees it merged but the
        // agent is still present from last frame, so nothing moves yet).
        app.apply(BridgeEvent::WorkstreamStatus {
            id: ids[0],
            status: WorkstreamStatus::Merged,
        });
        scene.sync(&app, 1.0, true);
        assert_eq!(
            scene.consoles[0].ws,
            Some(ids[0]),
            "slot holds Merged green while its agent is still walking out"
        );
        assert_eq!(
            scene.consoles[0].visual,
            crate::deck::sprites::ConsoleVisual::Merged
        );

        // Next sync: the departed workstream's agent is gone, so its slot
        // frees and goes to the earliest unslotted, still-live workstream.
        scene.sync(&app, 1.1, true);
        assert_eq!(
            scene.consoles[0].ws,
            Some(ids[8]),
            "freed slot goes to the earliest unslotted workstream"
        );
        assert_eq!(scene.overflow, 0);
        assert!(
            scene.agents.iter().any(|a| a.ws == ids[8]),
            "the ninth workstream gets a walk-in agent once it is slotted"
        );
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

    #[test]
    fn provision_spawns_walking_agent_whose_path_ends_at_console() {
        let app = app_with(&[WorkstreamStatus::Working]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, false);
        assert_eq!(scene.agents.len(), 1);
        let a = &scene.agents[0];
        assert_eq!(a.phase, AgentPhase::Walking);
        assert_eq!(*a.path.last().unwrap(), stand_point(0));
        // starts at the turbolift
        assert!(LIFT.contains(a.x, a.y));
    }

    #[test]
    fn reduce_motion_places_agent_instantly() {
        let app = app_with(&[WorkstreamStatus::Working]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        let a = &scene.agents[0];
        assert_eq!(a.phase, AgentPhase::Seated);
        assert_eq!((a.x, a.y), stand_point(0));
    }

    #[test]
    fn walking_agent_reaches_console_over_time() {
        let app = app_with(&[WorkstreamStatus::Working]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, false);
        // Walk far longer than any path needs at 34 px/s.
        let mut t = 0.0;
        for _ in 0..600 {
            t += 0.1;
            scene.sync(&app, t, false);
        }
        let a = &scene.agents[0];
        assert_eq!(a.phase, AgentPhase::Seated);
        assert_eq!((a.x, a.y), stand_point(0));
    }

    #[test]
    fn merge_sends_agent_leaving_then_gone() {
        let mut app = app_with(&[WorkstreamStatus::Working]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true); // seated instantly
        let id = app.workstream_order[0];
        app.apply(BridgeEvent::WorkstreamStatus {
            id,
            status: WorkstreamStatus::Merged,
        });
        scene.sync(&app, 1.0, true);
        assert!(scene.agents.is_empty(), "reduce_motion removes instantly");

        // And with motion: phase becomes Leaving.
        let mut app2 = app_with(&[WorkstreamStatus::Working]);
        let mut scene2 = SceneState::default();
        scene2.sync(&app2, 0.0, false);
        let mut t = 0.0;
        for _ in 0..600 {
            t += 0.1;
            scene2.sync(&app2, t, false);
        }
        let id2 = app2.workstream_order[0];
        app2.apply(BridgeEvent::WorkstreamStatus {
            id: id2,
            status: WorkstreamStatus::Merged,
        });
        scene2.sync(&app2, t + 0.1, false);
        assert_eq!(scene2.agents[0].phase, AgentPhase::Leaving);
    }

    #[test]
    fn breach_raises_alarm_that_expires() {
        let mut app = app_with(&[WorkstreamStatus::Working]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        let id = app.workstream_order[0];
        app.apply(BridgeEvent::WorkstreamStatus {
            id,
            status: WorkstreamStatus::Breached { round: 1 },
        });
        scene.sync(&app, 1.0, true);
        assert!(scene.agents[0].alarm_until > 1.0);
        scene.sync(&app, 10.0, true);
        assert!(scene.agents[0].alarm_until < 10.0, "alarm expired");
    }

    #[test]
    fn hook_denial_blinks_tactical() {
        use bridge_core::{DecisionKind, DecisionSource, HookDecisionRecord};
        let mut app = app_with(&[WorkstreamStatus::Working]);
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);
        assert_eq!(scene.tactical_blink_until, 0.0);
        app.apply(BridgeEvent::HookDecision(HookDecisionRecord {
            timestamp: chrono::Utc::now(),
            workstream: app.workstream_order[0],
            hook_event: "PreToolUse".into(),
            tool_name: Some("Bash".into()),
            decision: DecisionKind::Deny,
            reason: Some("outside worktree".into()),
            rule: "path-policy".into(),
            source: DecisionSource::PrimeDirective,
            latency_ms: 3,
        }));
        scene.sync(&app, 2.0, true);
        assert!(scene.tactical_blink_until > 2.0);
    }

    #[test]
    fn tactical_blink_survives_feed_saturating_past_max_feed() {
        use bridge_core::{DecisionKind, DecisionSource, HookDecisionRecord};
        let mut app = app_with(&[WorkstreamStatus::Working]);
        let ws = app.workstream_order[0];
        let mut scene = SceneState::default();
        scene.sync(&app, 0.0, true);

        let base = chrono::Utc::now();
        for i in 0..(crate::state::MAX_FEED + 5) {
            app.apply(BridgeEvent::HookDecision(HookDecisionRecord {
                timestamp: base + chrono::Duration::milliseconds(i as i64),
                workstream: ws,
                hook_event: "PreToolUse".into(),
                tool_name: Some("Bash".into()),
                decision: DecisionKind::Allow,
                reason: None,
                rule: format!("r{i}"),
                source: DecisionSource::Passthrough,
                latency_ms: 1,
            }));
        }
        assert_eq!(
            app.tactical_feed.len(),
            crate::state::MAX_FEED,
            "feed is bounded"
        );
        // Sync once while the feed is already saturated at MAX_FEED - this
        // is what used to permanently disarm the len-based cursor.
        scene.sync(&app, 1.0, true);
        assert_eq!(scene.tactical_blink_until, 0.0);

        app.apply(BridgeEvent::HookDecision(HookDecisionRecord {
            timestamp: base + chrono::Duration::milliseconds((crate::state::MAX_FEED + 100) as i64),
            workstream: ws,
            hook_event: "PreToolUse".into(),
            tool_name: Some("Bash".into()),
            decision: DecisionKind::Deny,
            reason: Some("outside worktree".into()),
            rule: "path-policy".into(),
            source: DecisionSource::PrimeDirective,
            latency_ms: 3,
        }));
        assert_eq!(
            app.tactical_feed.len(),
            crate::state::MAX_FEED,
            "still bounded after the deny push"
        );
        scene.sync(&app, 2.0, true);
        assert!(
            scene.tactical_blink_until > 2.0,
            "blink must fire even though the feed length never grew past MAX_FEED"
        );
    }
}
