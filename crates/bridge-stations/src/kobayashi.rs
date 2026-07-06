//! Kobayashi Maru: the adversarial tester.
//!
//! Every completed workstream faces it BEFORE entering the merge queue.
//! Isolation invariants (enforced here + by Tactical path policy):
//! - fresh throwaway worktree at the branch head, per round
//! - FRESH session per round (never --resume; no memory of prior attempts)
//! - writes only under tests/adversarial/
//! - it can never modify the implementation it attacks.

use crate::ports::{ComputerPort, GitPort, TacticalPort, TurnPort};
use bridge_core::{BattleReport, ClaudeConfig, WorkstreamSpec};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum KobayashiError {
    #[error("tester turn failed: {0}")]
    Turn(String),
    #[error("tester produced no parseable battle report")]
    NoReport,
    #[error(transparent)]
    Git(#[from] bridge_git::GitError),
}

pub struct KobayashiRunner;

impl KobayashiRunner {
    /// Run one round against `ws`'s branch:
    /// 1. provision a throwaway worktree (engineering::provision_throwaway),
    /// 2. gather changed files + prior findings from the computer,
    /// 3. one claude run: kobayashi prompt, BattleReport::json_schema
    ///    structured output, kobayashi model, FRESH session,
    /// 4. harvest committed test commits (commits_ahead of branch head)
    ///    and cherry-pick them into the implementation worktree,
    /// 5. stamp + record the BattleReport (consistency-checked: an
    ///    inconsistent report becomes Breached with a synthetic finding,
    ///    fail safe),
    /// 6. decommission the throwaway (always, even on error).
    pub async fn run_round<D>(
        deps: &D,
        claude_cfg: &ClaudeConfig,
        helper_path: &Path,
        ws: &WorkstreamSpec,
        mission_slug: &str,
        round: u32,
    ) -> Result<BattleReport, KobayashiError>
    where
        D: TurnPort + GitPort + TacticalPort + ComputerPort,
    {
        todo!()
    }
}
