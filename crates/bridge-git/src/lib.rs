//! Git worktree lifecycle and merge primitives.
//!
//! All operations shell out to the `git` CLI with explicit `-C <path>`;
//! nothing here ever touches the index of the user's primary checkout.
//! Worktrees live under a root OUTSIDE the target repository so agents
//! cannot see or clobber each other.

use bridge_core::MergeMode;
use std::path::{Path, PathBuf};
use std::process::Output;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git {args:?} failed with status {status}: {stderr}")]
    Command {
        args: Vec<String>,
        status: i32,
        stderr: String,
    },
    #[error("failed to run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A live worktree checked out on its own branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeHandle {
    pub path: PathBuf,
    pub branch: String,
}

/// Owns worktree creation/removal and branch operations for one target repo.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    repo_root: PathBuf,
    worktrees_root: PathBuf,
}

impl WorktreeManager {
    /// `repo_root` must be an existing git repository work dir;
    /// `worktrees_root` must NOT be inside it (validated here).
    pub fn new(repo_root: PathBuf, worktrees_root: PathBuf) -> Result<Self, GitError> {
        todo!()
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn worktrees_root(&self) -> &Path {
        &self.worktrees_root
    }

    /// Create `<worktrees_root>/<mission_slug>/<ws_slug>` on a new branch
    /// `bridge/<mission_slug>/<ws_slug>` starting at `base_ref`. Fails if
    /// the branch already exists.
    pub fn create(
        &self,
        mission_slug: &str,
        ws_slug: &str,
        base_ref: &str,
    ) -> Result<WorktreeHandle, GitError> {
        todo!()
    }

    /// Fresh throwaway worktree (detached HEAD) at the head of `branch`,
    /// under `<worktrees_root>/throwaway/<random>`. Used per Kobayashi run
    /// so the tester never mutates the implementation worktree.
    pub fn create_throwaway(&self, branch: &str) -> Result<WorktreeHandle, GitError> {
        todo!()
    }

    /// `git worktree remove` (+ `--force` when asked); prunes bookkeeping.
    /// Does NOT delete the branch.
    pub fn remove(&self, handle: &WorktreeHandle, force: bool) -> Result<(), GitError> {
        todo!()
    }

    /// Delete a branch (used for throwaway/cleanup paths only).
    pub fn delete_branch(&self, branch: &str, force: bool) -> Result<(), GitError> {
        todo!()
    }

    /// Rebase the worktree's branch onto `target`. On conflict the rebase
    /// is ABORTED before returning so the worktree is always left clean;
    /// the conflicting file list is captured before aborting.
    pub fn rebase_onto(&self, handle: &WorktreeHandle, target: &str) -> Result<RebaseOutcome, GitError> {
        todo!()
    }

    /// Merge `branch` into the repository's main branch, locally only.
    /// `MergeMode::Rebase` requires `branch` to already be a descendant of
    /// main (rebase first via `rebase_onto`) and fast-forwards; `MergeCommit`
    /// creates a merge commit. Refuses to run if main's worktree is dirty.
    /// NEVER pushes.
    pub fn merge_into_main(&self, branch: &str, mode: MergeMode) -> Result<MergeOutcome, GitError> {
        todo!()
    }

    /// Name of the repository's main branch ("main" or "master"), detected
    /// once from `origin/HEAD` falling back to local branch existence.
    pub fn main_branch(&self) -> Result<String, GitError> {
        todo!()
    }

    /// Append ignore patterns to the WORKTREE-SPECIFIC exclude file
    /// (`.git/worktrees/<name>/info/exclude`) so `.claude/` settings never
    /// show up in `git status` or get committed by an agent.
    pub fn add_worktree_exclude(&self, handle: &WorktreeHandle, patterns: &[&str]) -> Result<(), GitError> {
        todo!()
    }

    /// Current head commit of a ref.
    pub fn rev_parse(&self, reference: &str) -> Result<String, GitError> {
        todo!()
    }

    /// `git diff --stat <main>...<branch>` for merge proposals.
    pub fn diff_stat_against_main(&self, branch: &str) -> Result<String, GitError> {
        todo!()
    }

    /// `git diff --name-only <main>...<branch>`: the files a workstream
    /// touched, used to index Kobayashi findings and fetch past ones.
    pub fn changed_files_against_main(&self, branch: &str) -> Result<Vec<PathBuf>, GitError> {
        todo!()
    }

    /// Commits on `branch` that are not on `base` (oldest first), for
    /// harvesting Kobayashi test commits from a throwaway worktree.
    pub fn commits_ahead(&self, branch: &str, base: &str) -> Result<Vec<String>, GitError> {
        todo!()
    }

    /// Cherry-pick commits (oldest first) inside a worktree. On conflict,
    /// aborts the cherry-pick and returns the conflicting commit.
    pub fn cherry_pick(&self, handle: &WorktreeHandle, commits: &[String]) -> Result<Result<(), String>, GitError> {
        todo!()
    }

    /// Worktree paths registered under our root that no longer correspond
    /// to a live mission (orphan cleanup at startup). Detection: `git
    /// worktree list --porcelain` entries whose path is under
    /// `worktrees_root`.
    pub fn list_bridge_worktrees(&self) -> Result<Vec<PathBuf>, GitError> {
        todo!()
    }

    /// Run an arbitrary read-only git command in the repo (plumbing for
    /// stations that need e.g. `log`/`show`); rejects argv containing
    /// mutating subcommands.
    pub fn read_only(&self, args: &[&str]) -> Result<Output, GitError> {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    Clean,
    /// Conflicts were detected; the rebase was aborted and the worktree is
    /// back at its pre-rebase state.
    Conflicts { files: Vec<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// New head commit of main after the merge.
    pub main_head: String,
}
