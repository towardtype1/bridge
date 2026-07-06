//! The rule engine that adjudicates hook payloads.

use bridge_compat::HookPayload;
use bridge_core::{TacticalConfig, WorkstreamId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

/// Per-workstream context the engine needs for path policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkstreamCtx {
    pub worktree_path: PathBuf,
    /// Extra directories writes are allowed in (e.g. `tests/adversarial`
    /// RELATIVE to the worktree for Kobayashi Maru; for that station this
    /// is also the ONLY writable prefix).
    pub write_only_under: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    /// No rule fired. The server responds passthrough `{}` so the static
    /// permission gate still applies. `suspicious` marks candidates for
    /// the optional deep scan.
    Pass { suspicious: bool },
    Deny {
        reason: String,
        rule: String,
    },
    Escalate {
        question: String,
        rule: String,
    },
}

pub struct PolicyEngine {
    _config: TacticalConfig,
    _red_alert: AtomicBool,
    _workstreams: RwLock<HashMap<WorkstreamId, WorkstreamCtx>>,
}

impl PolicyEngine {
    /// Compiles config regexes once; invalid patterns are an error here,
    /// not at decision time.
    pub fn new(config: TacticalConfig) -> Result<Self, regex::Error> {
        todo!()
    }

    pub fn register_workstream(&self, id: WorkstreamId, ctx: WorkstreamCtx) {
        todo!()
    }

    pub fn unregister_workstream(&self, id: WorkstreamId) {
        todo!()
    }

    pub fn set_red_alert(&self, active: bool) {
        todo!()
    }

    pub fn red_alert(&self) -> bool {
        todo!()
    }

    /// Adjudicate one hook payload for a registered workstream.
    ///
    /// PreToolUse rules (in decision order):
    /// - PRIME: file tools (Edit/Write/NotebookEdit, and Read of protected
    ///   paths) must stay inside the worktree (+ `write_only_under`
    ///   restriction when set: writes outside that prefix deny).
    /// - PRIME: Bash worktree escape heuristics: absolute paths outside
    ///   the worktree in write positions, `cd` out of tree followed by
    ///   mutating commands, `git -C` pointing outside.
    /// - PRIME: credential reads (protected_path_patterns hits), `git push
    ///   --force`, `curl|sh` style pipes, `rm -rf` targeting paths at or
    ///   above the worktree root. These DENY regardless of config.
    /// - CONFIG: destructive_patterns regex hits deny.
    /// - CONFIG: network egress commands (curl/wget/nc/ssh/scp) escalate
    ///   when `allow_network_egress` is false. `git push` (non-force)
    ///   always escalates (remote push is never automatic).
    /// - MCP: `mcp__linear__*` reads pass; writes escalate unless they
    ///   match an expected-sync shape the caller registered; deletes
    ///   always escalate; under Red Alert all MCP writes escalate.
    /// - RED ALERT: anything not already denied escalates.
    /// - Otherwise `Pass`, with `suspicious` true for calls that matched a
    ///   "watch" heuristic (base64/eval in Bash, unusually long single
    ///   commands, file writes to dotfiles).
    ///
    /// PostToolUse/Stop payloads always `Pass` (observability only), but
    /// still get logged upstream.
    ///
    /// Unregistered workstream = Deny (fail closed).
    pub fn decide(&self, ws: WorkstreamId, payload: &HookPayload) -> PolicyVerdict {
        todo!()
    }
}
