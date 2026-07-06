//! Orders: a single prompt dispatched to a station agent.

use crate::ids::{OrderId, Station, WorkstreamId};
use serde::{Deserialize, Serialize};

/// One unit of delegated work. Tactical screens the order text and tool
/// lists before any claude process spawns; the engine enforces max_turns
/// via `--max-turns` and the tool lists via `--allowedTools` /
/// `--disallowedTools`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub workstream: WorkstreamId,
    pub station: Station,
    pub prompt: String,
    pub max_turns: u32,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// Model override; None = station profile default.
    pub model: Option<String>,
}

impl Order {
    pub fn new(workstream: WorkstreamId, station: Station, prompt: impl Into<String>) -> Self {
        Self {
            id: OrderId::new(),
            workstream,
            station,
            prompt: prompt.into(),
            max_turns: 10,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            model: None,
        }
    }
}
