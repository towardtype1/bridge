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

/// The full Linear MCP tool surface, granted to the Captain when Linear
/// sync is configured. Server-level rule: covers every tool the server
/// exposes (per Claude Code MCP permission syntax `mcp__<server>`).
const LINEAR_FULL: &[&str] = &["mcp__linear"];

/// Read-only Linear MCP tools, granted to Science when Linear is enabled.
const LINEAR_READ_ONLY: &[&str] = &[
    "mcp__linear__list_issues",
    "mcp__linear__get_issue",
    "mcp__linear__list_my_issues",
    "mcp__linear__list_issue_statuses",
    "mcp__linear__get_issue_status",
    "mcp__linear__list_issue_labels",
    "mcp__linear__list_comments",
    "mcp__linear__list_cycles",
    "mcp__linear__list_projects",
    "mcp__linear__get_project",
    "mcp__linear__list_teams",
    "mcp__linear__get_team",
    "mcp__linear__list_users",
    "mcp__linear__get_user",
    "mcp__linear__list_documents",
    "mcp__linear__get_document",
    "mcp__linear__search_documentation",
];

/// Read-only Bash permission rules for Science.
const SCIENCE_BASH_READ_ONLY: &[&str] = &[
    "Bash(git log *)",
    "Bash(git show *)",
    "Bash(git diff *)",
    "Bash(grep *)",
    "Bash(rg *)",
    "Bash(find *)",
    "Bash(ls *)",
    "Bash(cat *)",
];

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
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
    let default_model = config.claude.model.clone();
    let linear_enabled = config.linear.is_some();
    match station {
        Station::Captain => {
            let mut allowed = strings(&["Read"]);
            if linear_enabled {
                allowed.extend(strings(LINEAR_FULL));
            }
            StationProfile {
                station,
                append_system_prompt: "You are the Captain of a starship bridge crew running a \
                    software mission. You plan and delegate; you never edit files or run \
                    commands yourself. Decompose the objective into independent workstreams \
                    that specialist agents execute in isolated git worktrees. You plan in \
                    conversation with your commanding officer: discuss, question, and revise; \
                    propose a plan only through the structured proposed_plan field; never \
                    launch anything yourself."
                    .into(),
                allowed_tools: allowed,
                disallowed_tools: strings(&["Edit", "Write", "NotebookEdit", "Bash"]),
                model: default_model,
                max_turns_default: 16,
            }
        }
        Station::Helm => StationProfile {
            station,
            append_system_prompt: "You are the Helm officer: you execute one workstream inside \
                your assigned git worktree. Stay inside the worktree, follow the brief exactly, \
                write tests for what you build, and commit your work with clear messages."
                .into(),
            allowed_tools: strings(&["Edit", "Write", "Read", "NotebookEdit", "TodoWrite", "Bash"]),
            disallowed_tools: Vec::new(),
            model: default_model,
            max_turns_default: 40,
        },
        Station::Science => {
            let mut allowed = strings(&["Read", "WebFetch", "WebSearch"]);
            allowed.extend(strings(SCIENCE_BASH_READ_ONLY));
            if linear_enabled {
                allowed.extend(strings(LINEAR_READ_ONLY));
            }
            StationProfile {
                station,
                append_system_prompt: "You are the Science officer: strictly read-only research \
                    and analysis. You never modify anything; you read, search and report."
                    .into(),
                allowed_tools: allowed,
                disallowed_tools: strings(&["Edit", "Write", "NotebookEdit"]),
                model: default_model,
                max_turns_default: 20,
            }
        }
        Station::KobayashiMaru => StationProfile {
            station,
            append_system_prompt: "You are the Kobayashi Maru: an adversarial tester. Your job \
                is to BREAK the change under test, not to confirm it works. You may only write \
                files under tests/adversarial/; you can never modify the implementation."
                .into(),
            allowed_tools: strings(&["Read", "Bash", "Write", "Edit"]),
            disallowed_tools: strings(&["WebFetch", "WebSearch", "NotebookEdit"]),
            model: config.claude.kobayashi_model.clone(),
            max_turns_default: 30,
        },
        Station::Comms => StationProfile {
            station,
            append_system_prompt: "You are the Comms officer: you turn mission telemetry into a \
                clear, honest report for the user. You have no tools; format only what you are \
                given, never invent results."
                .into(),
            allowed_tools: Vec::new(),
            disallowed_tools: strings(&[
                "Edit",
                "Write",
                "NotebookEdit",
                "Bash",
                "WebFetch",
                "WebSearch",
                "Task",
                "TodoWrite",
            ]),
            model: default_model,
            max_turns_default: 1,
        },
        Station::Ops | Station::Engineering | Station::Tactical => {
            debug_assert!(
                false,
                "procedural station {station} never holds a model session"
            );
            StationProfile {
                station,
                append_system_prompt: String::new(),
                allowed_tools: Vec::new(),
                disallowed_tools: Vec::new(),
                model: default_model,
                max_turns_default: 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::LinearConfig;

    fn cfg() -> BridgeConfig {
        let mut c = BridgeConfig::default();
        c.claude.model = "test-default-model".into();
        c.claude.kobayashi_model = "test-kobayashi-model".into();
        c
    }

    fn cfg_with_linear() -> BridgeConfig {
        let mut c = cfg();
        c.linear = Some(LinearConfig {
            mcp_config_path: "/tmp/linear-mcp.json".into(),
        });
        c
    }

    const MUTATING: &[&str] = &["Edit", "Write", "NotebookEdit"];

    #[test]
    fn science_has_zero_mutating_tools_and_disallows_them() {
        let p = station_profile(Station::Science, &cfg_with_linear());
        for tool in &p.allowed_tools {
            for bad in MUTATING {
                assert!(
                    !tool.starts_with(bad),
                    "science allowed mutating tool {tool}"
                );
            }
            assert_ne!(tool, "Bash", "science must never get unrestricted Bash");
            if tool.starts_with("Bash") {
                assert!(
                    tool.starts_with("Bash("),
                    "science bash rule must be scoped: {tool}"
                );
            }
            if tool.starts_with("mcp__linear__") {
                assert!(
                    tool.contains("list") || tool.contains("get") || tool.contains("search"),
                    "science linear tool must be read-only: {tool}"
                );
            }
        }
        for bad in MUTATING {
            assert!(
                p.disallowed_tools.iter().any(|t| t == bad),
                "science must disallow {bad}"
            );
        }
    }

    #[test]
    fn science_bash_rules_are_the_read_only_set() {
        let p = station_profile(Station::Science, &cfg());
        for rule in SCIENCE_BASH_READ_ONLY {
            assert!(p.allowed_tools.iter().any(|t| t == rule), "missing {rule}");
        }
    }

    #[test]
    fn kobayashi_uses_the_configured_kobayashi_model() {
        let p = station_profile(Station::KobayashiMaru, &cfg());
        assert_eq!(p.model, "test-kobayashi-model");
        for tool in ["Read", "Bash", "Write", "Edit"] {
            assert!(p.allowed_tools.iter().any(|t| t == tool), "missing {tool}");
        }
    }

    #[test]
    fn comms_is_a_single_toolless_turn() {
        let p = station_profile(Station::Comms, &cfg());
        assert_eq!(p.max_turns_default, 1);
        assert!(p.allowed_tools.is_empty());
        assert!(p.disallowed_tools.iter().any(|t| t == "Bash"));
    }

    #[test]
    fn captain_never_edits_or_runs_commands() {
        let p = station_profile(Station::Captain, &cfg());
        assert_eq!(p.allowed_tools, vec!["Read".to_string()]);
        for bad in ["Edit", "Write", "Bash"] {
            assert!(p.disallowed_tools.iter().any(|t| t == bad));
        }
    }

    #[test]
    fn captain_gets_linear_tools_only_when_configured() {
        let without = station_profile(Station::Captain, &cfg());
        assert!(
            !without
                .allowed_tools
                .iter()
                .any(|t| t.starts_with("mcp__linear"))
        );
        let with = station_profile(Station::Captain, &cfg_with_linear());
        assert!(
            with.allowed_tools
                .iter()
                .any(|t| t.starts_with("mcp__linear"))
        );
    }

    #[test]
    fn helm_has_the_full_execution_toolset() {
        let p = station_profile(Station::Helm, &cfg());
        for tool in ["Edit", "Write", "Read", "NotebookEdit", "TodoWrite", "Bash"] {
            assert!(p.allowed_tools.iter().any(|t| t == tool), "missing {tool}");
        }
        assert_eq!(p.model, "test-default-model");
    }

    #[test]
    fn agent_stations_use_default_model_and_have_prompts() {
        for station in [
            Station::Captain,
            Station::Helm,
            Station::Science,
            Station::Comms,
        ] {
            let p = station_profile(station, &cfg());
            assert_eq!(p.model, "test-default-model", "{station}");
            assert!(!p.append_system_prompt.is_empty(), "{station}");
            assert_eq!(p.station, station);
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "procedural station")]
    fn procedural_stations_panic_in_debug() {
        station_profile(Station::Ops, &cfg());
    }
}
