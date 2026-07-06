//! Git worktree lifecycle and merge primitives.
//!
//! All operations shell out to the `git` CLI with explicit `-C <path>`;
//! nothing here ever touches the index of the user's primary checkout.
//! Worktrees live under a root OUTSIDE the target repository so agents
//! cannot see or clobber each other.

use bridge_core::MergeMode;
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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

/// Subcommands `read_only` will run. Fail closed: anything not on this list
/// is rejected, including subcommands that are only sometimes mutating
/// (`branch`, `tag`, `stash`, `reflog`, ...).
const READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "log",
    "show",
    "diff",
    "status",
    "rev-parse",
    "rev-list",
    "ls-files",
    "ls-tree",
    "cat-file",
    "blame",
    "grep",
    "shortlog",
    "describe",
    "merge-base",
    "show-ref",
    "for-each-ref",
    "name-rev",
    "check-ignore",
    "cherry",
    "count-objects",
    "var",
];

impl WorktreeManager {
    /// `repo_root` must be an existing git repository work dir;
    /// `worktrees_root` must NOT be inside it (validated here). Both paths
    /// are canonicalized; `worktrees_root` is created if missing.
    pub fn new(repo_root: PathBuf, worktrees_root: PathBuf) -> Result<Self, GitError> {
        let repo_root = fs::canonicalize(&repo_root)
            .map_err(|e| GitError::Invalid(format!("repo_root {}: {e}", repo_root.display())))?;
        // Resolve the actual repository toplevel; this also proves the path
        // is a git work dir.
        let out = checked_git(&repo_root, &["rev-parse", "--show-toplevel"])?;
        let repo_root = PathBuf::from(stdout_str(&out));

        let worktrees_root = normalize_absolute(&worktrees_root)?;
        if worktrees_root.starts_with(&repo_root) {
            return Err(GitError::Invalid(format!(
                "worktrees_root {} must not be inside the repository {}",
                worktrees_root.display(),
                repo_root.display()
            )));
        }
        if repo_root.starts_with(&worktrees_root) {
            return Err(GitError::Invalid(format!(
                "repository {} must not be inside worktrees_root {}",
                repo_root.display(),
                worktrees_root.display()
            )));
        }
        fs::create_dir_all(&worktrees_root).map_err(|e| {
            GitError::Invalid(format!(
                "cannot create worktrees_root {}: {e}",
                worktrees_root.display()
            ))
        })?;
        Ok(Self {
            repo_root,
            worktrees_root,
        })
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
        validate_slug("mission", mission_slug)?;
        validate_slug("workstream", ws_slug)?;
        let branch = format!("bridge/{mission_slug}/{ws_slug}");
        let branch_ref = format!("refs/heads/{branch}");
        let verify = raw_git(
            &self.repo_root,
            &["show-ref", "--verify", "--quiet", &branch_ref],
        )?;
        if verify.status.success() {
            return Err(GitError::Invalid(format!("branch {branch} already exists")));
        }
        let path = self.worktrees_root.join(mission_slug).join(ws_slug);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                GitError::Invalid(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        checked_git(
            &self.repo_root,
            &["worktree", "add", "-b", &branch, path_str(&path)?, base_ref],
        )?;
        tracing::debug!(branch, path = %path.display(), base_ref, "created worktree");
        Ok(WorktreeHandle { path, branch })
    }

    /// Fresh throwaway worktree (detached HEAD) at the head of `branch`,
    /// under `<worktrees_root>/throwaway/<random>`. Used per Kobayashi run
    /// so the tester never mutates the implementation worktree. The handle's
    /// `branch` field records the source branch the HEAD was taken from.
    pub fn create_throwaway(&self, branch: &str) -> Result<WorktreeHandle, GitError> {
        let dir = self.worktrees_root.join("throwaway");
        fs::create_dir_all(&dir)
            .map_err(|e| GitError::Invalid(format!("cannot create {}: {e}", dir.display())))?;
        let path = dir.join(random_token());
        checked_git(
            &self.repo_root,
            &["worktree", "add", "--detach", path_str(&path)?, branch],
        )?;
        tracing::debug!(branch, path = %path.display(), "created throwaway worktree");
        Ok(WorktreeHandle {
            path,
            branch: branch.to_string(),
        })
    }

    /// Resolve the HEAD commit of a specific worktree. Needed for detached
    /// worktrees (throwaways): after a Kobayashi tester commits, its work
    /// lives on a detached HEAD with no branch ref, so the source branch
    /// name cannot locate the new commits. This reads the actual HEAD of
    /// that worktree's own checkout.
    pub fn head_of(&self, handle: &WorktreeHandle) -> Result<String, GitError> {
        let out = checked_git(&handle.path, &["rev-parse", "HEAD"])?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `git worktree remove` (+ `--force` when asked); prunes bookkeeping.
    /// Does NOT delete the branch.
    pub fn remove(&self, handle: &WorktreeHandle, force: bool) -> Result<(), GitError> {
        let path = path_str(&handle.path)?;
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path);
        checked_git(&self.repo_root, &args)?;
        checked_git(&self.repo_root, &["worktree", "prune"])?;
        Ok(())
    }

    /// Delete a branch (used for throwaway/cleanup paths only).
    pub fn delete_branch(&self, branch: &str, force: bool) -> Result<(), GitError> {
        let flag = if force { "-D" } else { "-d" };
        checked_git(&self.repo_root, &["branch", flag, branch])?;
        Ok(())
    }

    /// Rebase the worktree's branch onto `target`. On conflict the rebase
    /// is ABORTED before returning so the worktree is always left clean;
    /// the conflicting file list is captured before aborting.
    pub fn rebase_onto(
        &self,
        handle: &WorktreeHandle,
        target: &str,
    ) -> Result<RebaseOutcome, GitError> {
        let args = ["rebase", target];
        let out = raw_git(&handle.path, &args)?;
        if out.status.success() {
            return Ok(RebaseOutcome::Clean);
        }
        if !self.rebase_in_progress(handle)? {
            // The rebase never started (bad target etc.); surface as-is.
            return Err(command_error(&handle.path, &args, &out));
        }
        let unmerged = checked_git(&handle.path, &["diff", "--name-only", "--diff-filter=U"])?;
        let files: Vec<PathBuf> = String::from_utf8_lossy(&unmerged.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .collect();
        checked_git(&handle.path, &["rebase", "--abort"])?;
        tracing::warn!(
            branch = handle.branch,
            target,
            ?files,
            "rebase conflict, aborted"
        );
        Ok(RebaseOutcome::Conflicts { files })
    }

    /// Merge `branch` into the repository's main branch, locally only.
    /// `MergeMode::Rebase` requires `branch` to already be a descendant of
    /// main (rebase first via `rebase_onto`) and fast-forwards; `MergeCommit`
    /// creates a merge commit (a conflicting merge is aborted before the
    /// error is returned). Refuses to run if main's worktree is dirty.
    /// NEVER pushes.
    pub fn merge_into_main(&self, branch: &str, mode: MergeMode) -> Result<MergeOutcome, GitError> {
        let main = self.main_branch()?;
        let main_wt = self
            .worktree_list()?
            .into_iter()
            .find(|(_, b)| b.as_deref() == Some(main.as_str()))
            .map(|(path, _)| path)
            .ok_or_else(|| {
                GitError::Invalid(format!("branch {main} is not checked out in any worktree"))
            })?;

        let status = checked_git(&main_wt, &["status", "--porcelain"])?;
        if !stdout_str(&status).is_empty() {
            return Err(GitError::Invalid(format!(
                "refusing to merge into {main}: checkout at {} is dirty",
                main_wt.display()
            )));
        }
        // Branch must resolve before we touch anything.
        self.rev_parse(&format!("refs/heads/{branch}"))?;

        match mode {
            MergeMode::Rebase => {
                let args = ["merge-base", "--is-ancestor", main.as_str(), branch];
                let ancestor = raw_git(&self.repo_root, &args)?;
                if !ancestor.status.success() {
                    if ancestor.status.code() == Some(1) {
                        return Err(GitError::Invalid(format!(
                            "{branch} is not a descendant of {main}; rebase it first"
                        )));
                    }
                    return Err(command_error(&self.repo_root, &args, &ancestor));
                }
                checked_git(&main_wt, &["merge", "--ff-only", branch])?;
            }
            MergeMode::MergeCommit => {
                let msg = format!("Merge branch '{branch}' into {main}");
                let args = ["merge", "--no-ff", "-m", msg.as_str(), branch];
                let out = raw_git(&main_wt, &args)?;
                if !out.status.success() {
                    // Leave the user's main checkout clean, then report.
                    let _ = raw_git(&main_wt, &["merge", "--abort"]);
                    return Err(command_error(&main_wt, &args, &out));
                }
            }
        }
        let head = checked_git(&main_wt, &["rev-parse", "HEAD"])?;
        Ok(MergeOutcome {
            main_head: stdout_str(&head),
        })
    }

    /// Name of the repository's main branch ("main" or "master"), detected
    /// once from `origin/HEAD` falling back to local branch existence.
    pub fn main_branch(&self) -> Result<String, GitError> {
        let sym = raw_git(
            &self.repo_root,
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        )?;
        if sym.status.success()
            && let Some(name) = stdout_str(&sym).strip_prefix("refs/remotes/origin/")
            && !name.is_empty()
        {
            return Ok(name.to_string());
        }
        for candidate in ["main", "master"] {
            let branch_ref = format!("refs/heads/{candidate}");
            let verify = raw_git(
                &self.repo_root,
                &["show-ref", "--verify", "--quiet", &branch_ref],
            )?;
            if verify.status.success() {
                return Ok(candidate.to_string());
            }
        }
        Err(GitError::Invalid(
            "could not detect main branch: no origin/HEAD and neither 'main' nor 'master' exists"
                .to_string(),
        ))
    }

    /// Append ignore patterns to the exclude file the worktree's own
    /// plumbing points at (`git rev-parse --git-path info/exclude` run FROM
    /// THE WORKTREE) so `.claude/` settings never show up in `git status`
    /// or get committed by an agent. NOTE: current git resolves this to the
    /// repository's shared `.git/info/exclude` (linked worktrees have no
    /// per-worktree exclude that git reads), so patterns are deduplicated
    /// before appending.
    pub fn add_worktree_exclude(
        &self,
        handle: &WorktreeHandle,
        patterns: &[&str],
    ) -> Result<(), GitError> {
        let out = checked_git(&handle.path, &["rev-parse", "--git-path", "info/exclude"])?;
        let reported = PathBuf::from(stdout_str(&out));
        let exclude = if reported.is_relative() {
            handle.path.join(reported)
        } else {
            reported
        };
        if let Some(parent) = exclude.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                GitError::Invalid(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let existing = fs::read_to_string(&exclude).unwrap_or_default();
        let mut body = String::new();
        for pattern in patterns {
            let already = existing.lines().chain(body.lines()).any(|l| l == *pattern);
            if !already {
                body.push_str(pattern);
                body.push('\n');
            }
        }
        if body.is_empty() {
            return Ok(());
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)
            .map_err(|e| GitError::Invalid(format!("cannot open {}: {e}", exclude.display())))?;
        // Ensure we start on a fresh line even if the file lacks a trailing
        // newline.
        if !existing.is_empty() && !existing.ends_with('\n') {
            body.insert(0, '\n');
        }
        file.write_all(body.as_bytes())
            .map_err(|e| GitError::Invalid(format!("cannot write {}: {e}", exclude.display())))?;
        Ok(())
    }

    /// Current head commit of a ref.
    pub fn rev_parse(&self, reference: &str) -> Result<String, GitError> {
        let out = checked_git(&self.repo_root, &["rev-parse", "--verify", reference])?;
        Ok(stdout_str(&out))
    }

    /// `git diff --stat <main>...<branch>` for merge proposals.
    pub fn diff_stat_against_main(&self, branch: &str) -> Result<String, GitError> {
        let main = self.main_branch()?;
        let range = format!("{main}...{branch}");
        let out = checked_git(&self.repo_root, &["diff", "--stat", &range])?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `git diff --name-only <main>...<branch>`: the files a workstream
    /// touched, used to index Kobayashi findings and fetch past ones.
    pub fn changed_files_against_main(&self, branch: &str) -> Result<Vec<PathBuf>, GitError> {
        let main = self.main_branch()?;
        let range = format!("{main}...{branch}");
        let out = checked_git(&self.repo_root, &["diff", "--name-only", &range])?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .collect())
    }

    /// Commits on `branch` that are not on `base` (oldest first), for
    /// harvesting Kobayashi test commits from a throwaway worktree.
    pub fn commits_ahead(&self, branch: &str, base: &str) -> Result<Vec<String>, GitError> {
        let range = format!("{base}..{branch}");
        let out = checked_git(&self.repo_root, &["rev-list", "--reverse", &range])?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Cherry-pick commits (oldest first) inside a worktree. On conflict,
    /// aborts the whole cherry-pick sequence (already-applied commits are
    /// rolled back too) and returns the conflicting commit as the inner
    /// `Err`.
    pub fn cherry_pick(
        &self,
        handle: &WorktreeHandle,
        commits: &[String],
    ) -> Result<Result<(), String>, GitError> {
        if commits.is_empty() {
            return Ok(Ok(()));
        }
        let mut args: Vec<&str> = vec!["cherry-pick"];
        args.extend(commits.iter().map(String::as_str));
        let out = raw_git(&handle.path, &args)?;
        if out.status.success() {
            return Ok(Ok(()));
        }
        let pick_head = raw_git(
            &handle.path,
            &["rev-parse", "--verify", "--quiet", "CHERRY_PICK_HEAD"],
        )?;
        if pick_head.status.success() {
            let conflicting = stdout_str(&pick_head);
            checked_git(&handle.path, &["cherry-pick", "--abort"])?;
            tracing::warn!(
                branch = handle.branch,
                commit = conflicting,
                "cherry-pick conflict, aborted"
            );
            return Ok(Err(conflicting));
        }
        Err(command_error(&handle.path, &args, &out))
    }

    /// Worktree paths registered under our root that no longer correspond
    /// to a live mission (orphan cleanup at startup). Detection: `git
    /// worktree list --porcelain` entries whose path is under
    /// `worktrees_root`.
    pub fn list_bridge_worktrees(&self) -> Result<Vec<PathBuf>, GitError> {
        Ok(self
            .worktree_list()?
            .into_iter()
            .map(|(path, _)| path)
            .filter(|path| path.starts_with(&self.worktrees_root))
            .collect())
    }

    /// Run an arbitrary read-only git command in the repo (plumbing for
    /// stations that need e.g. `log`/`show`); rejects argv containing
    /// mutating subcommands. Enforced fail-closed as an allowlist (`log`,
    /// `show`, `diff`, `status`, `rev-parse`, ...): anything not on it is
    /// rejected, as are global options before the subcommand and
    /// `--output*` flags that write files.
    pub fn read_only(&self, args: &[&str]) -> Result<Output, GitError> {
        let Some(sub) = args.first() else {
            return Err(GitError::Invalid("empty git argv".to_string()));
        };
        if sub.starts_with('-') {
            return Err(GitError::Invalid(format!(
                "global git options are not allowed in read_only: {sub}"
            )));
        }
        if !READ_ONLY_SUBCOMMANDS.iter().any(|allowed| allowed == sub) {
            return Err(GitError::Invalid(format!(
                "git subcommand {sub:?} is not on the read-only allowlist"
            )));
        }
        for arg in &args[1..] {
            if *arg == "--output" || arg.starts_with("--output=") || *arg == "-o" {
                return Err(GitError::Invalid(format!(
                    "flag {arg:?} writes files and is not allowed in read_only"
                )));
            }
        }
        checked_git(&self.repo_root, args)
    }

    /// True when the worktree has a rebase in progress (either the merge or
    /// the apply backend).
    fn rebase_in_progress(&self, handle: &WorktreeHandle) -> Result<bool, GitError> {
        for state_dir in ["rebase-merge", "rebase-apply"] {
            let out = checked_git(&handle.path, &["rev-parse", "--git-path", state_dir])?;
            let reported = PathBuf::from(stdout_str(&out));
            let path = if reported.is_relative() {
                handle.path.join(reported)
            } else {
                reported
            };
            if path.exists() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// All registered worktrees of the repo as `(path, checked-out branch)`;
    /// the branch is `None` for detached HEADs and bare entries.
    fn worktree_list(&self) -> Result<Vec<(PathBuf, Option<String>)>, GitError> {
        let out = checked_git(&self.repo_root, &["worktree", "list", "--porcelain"])?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut entries: Vec<(PathBuf, Option<String>)> = Vec::new();
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                entries.push((PathBuf::from(path), None));
            } else if let Some(branch) = line.strip_prefix("branch ")
                && let Some((_, slot)) = entries.last_mut()
            {
                *slot = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string(),
                );
            }
        }
        Ok(entries)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    Clean,
    /// Conflicts were detected; the rebase was aborted and the worktree is
    /// back at its pre-rebase state.
    Conflicts {
        files: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// New head commit of main after the merge.
    pub main_head: String,
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Run git with `-C <dir>` using the caller's normal environment, without
/// checking the exit status. Only spawn failures error here.
fn raw_git(dir: &Path, args: &[&str]) -> Result<Output, GitError> {
    tracing::trace!(dir = %dir.display(), ?args, "running git");
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    Ok(out)
}

/// Like [`raw_git`] but a non-zero exit becomes [`GitError::Command`] with
/// argv and stderr captured.
fn checked_git(dir: &Path, args: &[&str]) -> Result<Output, GitError> {
    let out = raw_git(dir, args)?;
    if !out.status.success() {
        return Err(command_error(dir, args, &out));
    }
    Ok(out)
}

fn command_error(dir: &Path, args: &[&str], out: &Output) -> GitError {
    let mut full = vec!["-C".to_string(), dir.display().to_string()];
    full.extend(args.iter().map(|a| a.to_string()));
    GitError::Command {
        args: full,
        status: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn path_str(path: &Path) -> Result<&str, GitError> {
    path.to_str()
        .ok_or_else(|| GitError::Invalid(format!("non-UTF-8 path: {}", path.display())))
}

/// Slugs become both branch name segments and directory names; keep them to
/// a conservative character set so they can never traverse paths or be
/// mistaken for flags.
fn validate_slug(kind: &str, slug: &str) -> Result<(), GitError> {
    let ok = !slug.is_empty()
        && !slug.starts_with('-')
        && !slug.starts_with('.')
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(GitError::Invalid(format!(
            "invalid {kind} slug {slug:?}: use only [A-Za-z0-9._-], not starting with '-' or '.'"
        )))
    }
}

/// Canonicalize an absolute path that may not exist yet: the deepest
/// existing ancestor is canonicalized and the missing tail re-appended.
fn normalize_absolute(path: &Path) -> Result<PathBuf, GitError> {
    if !path.is_absolute() {
        return Err(GitError::Invalid(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                cursor = parent;
            }
            _ => {
                return Err(GitError::Invalid(format!(
                    "cannot normalize path: {}",
                    path.display()
                )));
            }
        }
    }
    let mut out = fs::canonicalize(cursor)
        .map_err(|e| GitError::Invalid(format!("{}: {e}", cursor.display())))?;
    for name in missing.iter().rev() {
        out.push(name);
    }
    Ok(out)
}

/// Unique-enough token for throwaway worktree directory names: a randomly
/// seeded hash of the current time and pid (no extra dependency needed).
fn random_token() -> String {
    let mut hasher = RandomState::new().build_hasher();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    hasher.write_u128(now.as_nanos());
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}
