//! Identifier newtypes and the station roster.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

uuid_id!(
    /// A mission: one user objective decomposed into workstreams.
    MissionId
);
uuid_id!(
    /// A workstream: one branch + worktree + chain of orders.
    WorkstreamId
);
uuid_id!(
    /// A single order dispatched to a station agent.
    OrderId
);
uuid_id!(
    /// An escalation ticket awaiting a user decision in the GUI.
    EscalationId
);

/// A Claude Code CLI session identifier as reported in stream-json events.
///
/// Opaque to the harness; captured from `result.session_id` and passed back
/// via `--resume`, always from the same worktree directory (session lookup
/// is directory-scoped).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// The bridge stations. Each is an agent role with its own tool policy,
/// system prompt and model tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Station {
    /// Orchestrator; the only agent that talks to the user.
    Captain,
    /// Execution inside an assigned worktree.
    Helm,
    /// Read-only research and analysis.
    Science,
    /// Resource owner: semaphore, budgets, registries, scheduling.
    Ops,
    /// Harness operations: worktrees, hook installation, retries, cleanup.
    Engineering,
    /// Guardrails: policy engine and hook adjudication.
    Tactical,
    /// Adversarial tester of completed workstreams.
    KobayashiMaru,
    /// Output formatting and event streaming.
    Comms,
}

impl Station {
    pub const ALL: [Station; 8] = [
        Station::Captain,
        Station::Helm,
        Station::Science,
        Station::Ops,
        Station::Engineering,
        Station::Tactical,
        Station::KobayashiMaru,
        Station::Comms,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Station::Captain => "Captain",
            Station::Helm => "Helm",
            Station::Science => "Science",
            Station::Ops => "Ops",
            Station::Engineering => "Engineering",
            Station::Tactical => "Tactical",
            Station::KobayashiMaru => "Kobayashi Maru",
            Station::Comms => "Comms",
        }
    }
}

impl fmt::Display for Station {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_ids_round_trip_serde_as_plain_strings() {
        let id = WorkstreamId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.starts_with('"'));
        let back: WorkstreamId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn session_id_is_transparent() {
        let sid: SessionId =
            serde_json::from_str("\"a2356b0f-c205-48cc-8654-88c062868484\"").unwrap();
        assert_eq!(sid.0, "a2356b0f-c205-48cc-8654-88c062868484");
    }

    #[test]
    fn stations_have_stable_names() {
        assert_eq!(Station::KobayashiMaru.to_string(), "Kobayashi Maru");
        assert_eq!(Station::ALL.len(), 8);
    }
}
