//! Engineering: harness-ops procedures composed from the ports.
//!
//! Worktree provisioning = create worktree + install hook settings +
//! git-exclude the settings + register with Tactical. Decommission is the
//! reverse. Also owns retry policy for transient turn failures and the
//! startup orphan sweep.

use crate::ports::{GitPort, TacticalPort};
use bridge_core::{ClaudeConfig, WorkstreamId};
use bridge_git::{GitError, WorktreeHandle};
use bridge_tactical::WorkstreamCtx;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum EngineeringError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("failed to install hook settings: {0}")]
    Install(#[from] std::io::Error),
}

/// A fully provisioned workstream worktree: hooks installed, policy armed.
#[derive(Debug, Clone)]
pub struct ProvisionedWorktree {
    pub handle: WorktreeHandle,
    pub settings_path: PathBuf,
}

/// Create the worktree for (mission, workstream), arm Tactical, render
/// `.claude/settings.json` via `bridge_compat::render_worktree_settings`
/// with the issued token, write it 0600, and git-exclude `.claude/`.
/// `write_only_under` propagates the Kobayashi path restriction.
pub fn provision<G: GitPort, T: TacticalPort>(
    git: &G,
    tactical: &T,
    claude_cfg: &ClaudeConfig,
    helper_path: &Path,
    mission_slug: &str,
    ws: WorkstreamId,
    ws_slug: &str,
    base_ref: &str,
    write_only_under: Option<PathBuf>,
) -> Result<ProvisionedWorktree, EngineeringError> {
    todo!()
}

/// Same, but a throwaway worktree at `branch`'s head for one Kobayashi
/// round (fresh identity: new token, its own settings file).
pub fn provision_throwaway<G: GitPort, T: TacticalPort>(
    git: &G,
    tactical: &T,
    claude_cfg: &ClaudeConfig,
    helper_path: &Path,
    ws: WorkstreamId,
    branch: &str,
) -> Result<ProvisionedWorktree, EngineeringError> {
    todo!()
}

/// Disarm Tactical, delete the settings file, remove the worktree
/// (force = !keep). Errors are logged, not propagated: decommission is
/// best-effort cleanup.
pub fn decommission<G: GitPort, T: TacticalPort>(
    git: &G,
    tactical: &T,
    ws: WorkstreamId,
    provisioned: &ProvisionedWorktree,
    keep: bool,
) {
    todo!()
}

/// Locate the bundled helper binary: next to the current executable
/// (release layout) or under CARGO target dir (dev). Errors if missing so
/// missions cannot start without the veto path.
pub fn locate_helper() -> Result<PathBuf, EngineeringError> {
    todo!()
}
