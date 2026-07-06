//! Station profiles: tool policy, prompt fragment and model tier per station.

use bridge_core::{BridgeConfig, Station};

/// Static per-station policy applied to every order for that station.
#[derive(Debug, Clone, PartialEq)]
pub struct StationProfile {
    pub station: Station,
    /// Appended to the CLI's default system prompt.
    pub append_system_prompt: String,
    /// Passed via --allowedTools (permission rule syntax).
    pub allowed_tools: Vec<String>,
    /// Passed via --disallowedTools.
    pub disallowed_tools: Vec<String>,
    pub model: String,
    pub max_turns_default: u32,
}

/// Profile table (agent stations only; procedural stations panic in debug
/// via `debug_assert!` and return an empty Comms-like profile in release):
///
/// - Captain: plans and delegates; tools Read only (+ full `mcp__linear__*`
///   set when linear is configured); model = config default.
/// - Helm: Edit/Write/Read/NotebookEdit/TodoWrite/Bash; model = default.
/// - Science: strictly read-only: Read/WebFetch/WebSearch plus read-only
///   Bash rules (`Bash(git log *)`, `Bash(git show *)`, `Bash(git diff *)`,
///   `Bash(grep *)`, `Bash(rg *)`, `Bash(find *)`, `Bash(ls *)`,
///   `Bash(cat *)`) and read-only Linear tools when enabled; disallowed:
///   Edit/Write/NotebookEdit.
/// - KobayashiMaru: Read/Bash/Write/Edit (path-restricted by Tactical to
///   `tests/adversarial/` for writes); model = config.kobayashi_model.
/// - Comms: no tools (pure formatting turn, max_turns 1).
pub fn station_profile(station: Station, config: &BridgeConfig) -> StationProfile {
    todo!()
}
