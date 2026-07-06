//! Integration tests for bridge-git against real throwaway git repositories.
//!
//! Every git command spawned BY THE TESTS is hermetic: `GIT_CONFIG_NOSYSTEM=1`
//! and `HOME`/`XDG_CONFIG_HOME` pointed at a per-test temp dir. Each test repo
//! additionally carries local `user.name` / `user.email` / `commit.gpgsign` /
//! `core.hooksPath` config so that git commands spawned by the implementation
//! (which deliberately uses the caller's normal environment) also behave
//! deterministically regardless of the developer's global git config.

use bridge_core::MergeMode;
use bridge_git::{GitError, RebaseOutcome, WorktreeManager};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn git_raw(home: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("failed to spawn git")
}

fn git(home: &Path, dir: &Path, args: &[&str]) -> String {
    let out = git_raw(home, dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init_repo(home: &Path, base: &Path, name: &str, initial_branch: &str) -> PathBuf {
    let repo = base.join(name);
    git(home, base, &["init", "-q", "-b", initial_branch, repo.to_str().unwrap()]);
    git(home, &repo, &["config", "user.name", "Bridge Test"]);
    git(home, &repo, &["config", "user.email", "bridge@test.invalid"]);
    git(home, &repo, &["config", "commit.gpgsign", "false"]);
    let hooks = base.join("hooks-empty");
    fs::create_dir_all(&hooks).unwrap();
    git(home, &repo, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    repo
}

struct TestEnv {
    _tmp: TempDir,
    base: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    mgr: WorktreeManager,
}

impl TestEnv {
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        git(&self.home, dir, args)
    }

    fn git_raw(&self, dir: &Path, args: &[&str]) -> Output {
        git_raw(&self.home, dir, args)
    }

    fn commit_file(&self, dir: &Path, rel: &str, content: &str, msg: &str) -> String {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        self.git(dir, &["add", rel]);
        self.git(dir, &["commit", "-q", "-m", msg]);
        self.head(dir)
    }

    fn head(&self, dir: &Path) -> String {
        self.git(dir, &["rev-parse", "HEAD"]).trim().to_string()
    }

    fn status_porcelain(&self, dir: &Path) -> String {
        self.git(dir, &["status", "--porcelain"]).trim().to_string()
    }
}

fn setup() -> TestEnv {
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize so path comparisons survive the /var -> /private/var
    // symlink on macOS.
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let repo = init_repo(&home, &base, "repo", "main");
    git(&home, &repo, &["commit", "-q", "--allow-empty", "-m", "root"]);
    let seed = repo.join("seed.txt");
    fs::write(&seed, "seed line one\n").unwrap();
    git(&home, &repo, &["add", "seed.txt"]);
    git(&home, &repo, &["commit", "-q", "-m", "seed"]);
    let mgr = WorktreeManager::new(repo.clone(), base.join("wts")).expect("manager should build");
    TestEnv { _tmp: tmp, base, home, repo, mgr }
}

fn assert_invalid(err: &GitError, needle: &str) {
    match err {
        GitError::Invalid(msg) => assert!(
            msg.contains(needle),
            "expected Invalid message containing {needle:?}, got {msg:?}"
        ),
        other => panic!("expected GitError::Invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// new()
// ---------------------------------------------------------------------------

#[test]
fn new_rejects_worktrees_root_inside_repo_root() {
    let env = setup();
    let err = WorktreeManager::new(env.repo.clone(), env.repo.join("nested/wts")).unwrap_err();
    assert_invalid(&err, "inside");
}

#[test]
fn new_rejects_repo_root_inside_worktrees_root() {
    let env = setup();
    let err = WorktreeManager::new(env.repo.clone(), env.base.clone()).unwrap_err();
    assert_invalid(&err, "inside");
}

#[test]
fn new_rejects_non_repo_root() {
    let env = setup();
    let plain = env.base.join("plain");
    fs::create_dir_all(&plain).unwrap();
    let err = WorktreeManager::new(plain, env.base.join("wts2")).unwrap_err();
    assert!(
        matches!(err, GitError::Command { .. }),
        "expected Command error for non-repo root, got {err:?}"
    );
}

#[test]
fn new_rejects_relative_worktrees_root() {
    let env = setup();
    let err = WorktreeManager::new(env.repo.clone(), PathBuf::from("relative/wts")).unwrap_err();
    assert_invalid(&err, "absolute");
}

#[test]
fn new_canonicalizes_and_creates_worktrees_root() {
    let env = setup();
    assert!(env.mgr.worktrees_root().is_dir());
    assert!(env.mgr.worktrees_root().is_absolute());
    assert_eq!(env.mgr.repo_root(), env.repo.as_path());
}

// ---------------------------------------------------------------------------
// create() / create_throwaway()
// ---------------------------------------------------------------------------

#[test]
fn create_makes_branch_and_worktree_at_base_ref() {
    let env = setup();
    let base_sha = env.head(&env.repo);
    // Advance main so we can prove base_ref (not main head) is honored.
    env.commit_file(&env.repo, "later.txt", "later\n", "later");

    let handle = env.mgr.create("m1", "ws1", &base_sha).unwrap();
    assert_eq!(handle.branch, "bridge/m1/ws1");
    assert_eq!(handle.path, env.mgr.worktrees_root().join("m1").join("ws1"));
    assert!(handle.path.join("seed.txt").is_file());
    assert!(!handle.path.join("later.txt").exists(), "worktree must be at base_ref");

    let checked_out = env.git(&handle.path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(checked_out.trim(), "bridge/m1/ws1");
    assert_eq!(env.mgr.rev_parse("bridge/m1/ws1").unwrap(), base_sha);
}

#[test]
fn create_fails_if_branch_exists() {
    let env = setup();
    env.mgr.create("m1", "ws1", "main").unwrap();
    let err = env.mgr.create("m1", "ws1", "main").unwrap_err();
    assert_invalid(&err, "exists");
}

#[test]
fn create_rejects_path_traversal_slugs() {
    let env = setup();
    assert!(env.mgr.create("../evil", "ws1", "main").is_err());
    assert!(env.mgr.create("m1", "a/b", "main").is_err());
    assert!(env.mgr.create("", "ws1", "main").is_err());
    assert!(env.mgr.create("m1", "-flag", "main").is_err());
}

#[test]
fn create_throwaway_is_detached_at_branch_head() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    let ws_sha = env.commit_file(&handle.path, "ws.txt", "ws\n", "ws work");

    let throwaway = env.mgr.create_throwaway("bridge/m1/ws1").unwrap();
    assert!(
        throwaway.path.starts_with(env.mgr.worktrees_root().join("throwaway")),
        "throwaway path {} must live under <root>/throwaway/",
        throwaway.path.display()
    );
    assert_eq!(throwaway.branch, "bridge/m1/ws1");
    assert_eq!(env.head(&throwaway.path), ws_sha);
    let abbrev = env.git(&throwaway.path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(abbrev.trim(), "HEAD", "throwaway worktree must be detached");

    // Throwaways are removable like any other worktree.
    env.mgr.remove(&throwaway, false).unwrap();
    assert!(!throwaway.path.exists());
}

#[test]
fn create_throwaway_paths_are_unique() {
    let env = setup();
    env.mgr.create("m1", "ws1", "main").unwrap();
    let a = env.mgr.create_throwaway("bridge/m1/ws1").unwrap();
    let b = env.mgr.create_throwaway("bridge/m1/ws1").unwrap();
    assert_ne!(a.path, b.path);
}

// ---------------------------------------------------------------------------
// remove() / delete_branch()
// ---------------------------------------------------------------------------

#[test]
fn remove_deletes_worktree_but_keeps_branch() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    env.mgr.remove(&handle, false).unwrap();
    assert!(!handle.path.exists());
    let listing = env.git(&env.repo, &["worktree", "list", "--porcelain"]);
    assert!(!listing.contains(handle.path.to_str().unwrap()));
    // Branch survives removal.
    assert!(env.mgr.rev_parse("bridge/m1/ws1").is_ok());
}

#[test]
fn remove_dirty_worktree_requires_force() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    fs::write(handle.path.join("seed.txt"), "dirty\n").unwrap();

    let err = env.mgr.remove(&handle, false).unwrap_err();
    assert!(matches!(err, GitError::Command { .. }), "expected Command error, got {err:?}");
    assert!(handle.path.exists());

    env.mgr.remove(&handle, true).unwrap();
    assert!(!handle.path.exists());
}

#[test]
fn delete_branch_merged_and_unmerged() {
    let env = setup();

    // Merged (still at main head): plain delete works after worktree removal.
    let h1 = env.mgr.create("m1", "ws1", "main").unwrap();
    env.mgr.remove(&h1, false).unwrap();
    env.mgr.delete_branch("bridge/m1/ws1", false).unwrap();
    assert!(env.mgr.rev_parse("bridge/m1/ws1").is_err());

    // Unmerged: refuses without force, deletes with force.
    let h2 = env.mgr.create("m1", "ws2", "main").unwrap();
    env.commit_file(&h2.path, "ws2.txt", "x\n", "unmerged work");
    env.mgr.remove(&h2, false).unwrap();
    let err = env.mgr.delete_branch("bridge/m1/ws2", false).unwrap_err();
    assert!(matches!(err, GitError::Command { .. }), "expected Command error, got {err:?}");
    env.mgr.delete_branch("bridge/m1/ws2", true).unwrap();
    assert!(env.mgr.rev_parse("bridge/m1/ws2").is_err());
}

// ---------------------------------------------------------------------------
// rebase_onto()
// ---------------------------------------------------------------------------

#[test]
fn rebase_onto_clean_rewrites_parent() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    env.commit_file(&handle.path, "ws.txt", "ws\n", "ws work");
    let main_sha = env.commit_file(&env.repo, "main2.txt", "m2\n", "main advance");

    let outcome = env.mgr.rebase_onto(&handle, "main").unwrap();
    assert_eq!(outcome, RebaseOutcome::Clean);

    let parent = env.git(&handle.path, &["rev-parse", "HEAD~1"]);
    assert_eq!(parent.trim(), main_sha, "rebased commit must sit on top of main");
    assert!(handle.path.join("main2.txt").is_file());
    // Branch ref moved with the worktree HEAD.
    assert_eq!(env.mgr.rev_parse("bridge/m1/ws1").unwrap(), env.head(&handle.path));
}

#[test]
fn rebase_onto_conflict_reports_files_and_leaves_worktree_clean() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    let pre = env.commit_file(&handle.path, "seed.txt", "workstream version\n", "ws seed edit");
    env.commit_file(&env.repo, "seed.txt", "main version\n", "main seed edit");

    let outcome = env.mgr.rebase_onto(&handle, "main").unwrap();
    match outcome {
        RebaseOutcome::Conflicts { files } => {
            assert_eq!(files, vec![PathBuf::from("seed.txt")]);
        }
        other => panic!("expected conflicts, got {other:?}"),
    }

    // Worktree left clean, rebase aborted, branch back at pre-rebase head.
    assert_eq!(env.status_porcelain(&handle.path), "");
    assert_eq!(env.head(&handle.path), pre);
    assert_eq!(env.mgr.rev_parse("bridge/m1/ws1").unwrap(), pre);
    let abort = env.git_raw(&handle.path, &["rebase", "--abort"]);
    assert!(!abort.status.success(), "no rebase should be in progress after conflict handling");
}

// ---------------------------------------------------------------------------
// merge_into_main()
// ---------------------------------------------------------------------------

#[test]
fn merge_into_main_rebase_mode_fast_forwards() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    let ws_sha = env.commit_file(&handle.path, "ws.txt", "ws\n", "ws work");

    let outcome = env.mgr.merge_into_main("bridge/m1/ws1", MergeMode::Rebase).unwrap();
    assert_eq!(outcome.main_head, ws_sha);
    assert_eq!(env.mgr.rev_parse("main").unwrap(), ws_sha);
    // Fast-forward updated the main checkout's working tree.
    assert!(env.repo.join("ws.txt").is_file());
}

#[test]
fn merge_into_main_rebase_mode_refuses_non_descendant() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    env.commit_file(&handle.path, "ws.txt", "ws\n", "ws work");
    env.commit_file(&env.repo, "main2.txt", "m2\n", "main advance");

    let err = env.mgr.merge_into_main("bridge/m1/ws1", MergeMode::Rebase).unwrap_err();
    assert_invalid(&err, "descendant");
}

#[test]
fn merge_into_main_merge_commit_mode_creates_merge_commit() {
    let env = setup();
    let old_main = env.head(&env.repo);
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    let ws_sha = env.commit_file(&handle.path, "ws.txt", "ws\n", "ws work");

    let outcome = env.mgr.merge_into_main("bridge/m1/ws1", MergeMode::MergeCommit).unwrap();
    let main_head = env.mgr.rev_parse("main").unwrap();
    assert_eq!(outcome.main_head, main_head);
    let p1 = env.git(&env.repo, &["rev-parse", "main^1"]);
    let p2 = env.git(&env.repo, &["rev-parse", "main^2"]);
    assert_eq!(p1.trim(), old_main);
    assert_eq!(p2.trim(), ws_sha);
}

#[test]
fn merge_into_main_refuses_dirty_main_checkout() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    let pre_main = env.head(&env.repo);
    env.commit_file(&handle.path, "ws.txt", "ws\n", "ws work");
    fs::write(env.repo.join("seed.txt"), "uncommitted local edit\n").unwrap();

    let err = env.mgr.merge_into_main("bridge/m1/ws1", MergeMode::Rebase).unwrap_err();
    assert_invalid(&err, "dirty");
    assert_eq!(env.mgr.rev_parse("main").unwrap(), pre_main, "main must not move");
}

// ---------------------------------------------------------------------------
// main_branch()
// ---------------------------------------------------------------------------

#[test]
fn main_branch_detects_main() {
    let env = setup();
    assert_eq!(env.mgr.main_branch().unwrap(), "main");
}

#[test]
fn main_branch_detects_master() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let repo = init_repo(&home, &base, "repo", "master");
    let seed = repo.join("seed.txt");
    fs::write(&seed, "seed\n").unwrap();
    git(&home, &repo, &["add", "seed.txt"]);
    git(&home, &repo, &["commit", "-q", "-m", "seed"]);

    let mgr = WorktreeManager::new(repo, base.join("wts")).unwrap();
    assert_eq!(mgr.main_branch().unwrap(), "master");
}

#[test]
fn main_branch_prefers_origin_head() {
    let env = setup();
    env.git(
        &env.repo,
        &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk"],
    );
    assert_eq!(env.mgr.main_branch().unwrap(), "trunk");
}

// ---------------------------------------------------------------------------
// add_worktree_exclude()
// ---------------------------------------------------------------------------

#[test]
fn add_worktree_exclude_hides_files_from_status() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    fs::create_dir_all(handle.path.join(".claude")).unwrap();
    fs::write(handle.path.join(".claude/settings.json"), "{}\n").unwrap();

    assert!(env.status_porcelain(&handle.path).contains(".claude"));
    env.mgr.add_worktree_exclude(&handle, &[".claude/"]).unwrap();
    assert_eq!(env.status_porcelain(&handle.path), "", "excluded files must vanish from status");

    // The pattern landed exactly where the worktree's own plumbing points.
    let reported = env.git(&handle.path, &["rev-parse", "--git-path", "info/exclude"]);
    let reported = PathBuf::from(reported.trim());
    let exclude_path =
        if reported.is_relative() { handle.path.join(reported) } else { reported };
    let content = fs::read_to_string(&exclude_path).unwrap();
    assert!(content.contains(".claude/"));
}

#[test]
fn add_worktree_exclude_appends_without_duplicates() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    env.mgr.add_worktree_exclude(&handle, &[".claude/", "*.tmp"]).unwrap();
    env.mgr.add_worktree_exclude(&handle, &[".claude/"]).unwrap();

    let reported = env.git(&handle.path, &["rev-parse", "--git-path", "info/exclude"]);
    let reported = PathBuf::from(reported.trim());
    let exclude_path =
        if reported.is_relative() { handle.path.join(reported) } else { reported };
    let content = fs::read_to_string(&exclude_path).unwrap();
    assert_eq!(content.lines().filter(|l| *l == ".claude/").count(), 1);
    assert_eq!(content.lines().filter(|l| *l == "*.tmp").count(), 1);
}

// ---------------------------------------------------------------------------
// rev_parse() / diff helpers / commits_ahead()
// ---------------------------------------------------------------------------

#[test]
fn rev_parse_resolves_refs_and_errors_on_bad_ref() {
    let env = setup();
    let head = env.mgr.rev_parse("HEAD").unwrap();
    assert_eq!(head.len(), 40);
    assert!(head.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(head, env.head(&env.repo));

    let err = env.mgr.rev_parse("no-such-ref").unwrap_err();
    assert!(matches!(err, GitError::Command { .. }), "expected Command error, got {err:?}");
}

#[test]
fn diff_stat_against_main_reports_branch_changes() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    env.commit_file(&handle.path, "feature.rs", "fn f() {}\n", "add feature");

    let stat = env.mgr.diff_stat_against_main("bridge/m1/ws1").unwrap();
    assert!(stat.contains("feature.rs"), "stat was: {stat}");
    assert!(stat.contains("1 file changed"), "stat was: {stat}");
}

#[test]
fn changed_files_against_main_lists_only_branch_files() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    env.commit_file(&handle.path, "feature.rs", "fn f() {}\n", "add feature");
    env.commit_file(&handle.path, "docs/notes.md", "notes\n", "add notes");
    // Advance main on an unrelated file: three-dot diff must NOT include it.
    env.commit_file(&env.repo, "mainside.txt", "m\n", "main side");

    let mut files = env.mgr.changed_files_against_main("bridge/m1/ws1").unwrap();
    files.sort();
    assert_eq!(files, vec![PathBuf::from("docs/notes.md"), PathBuf::from("feature.rs")]);
}

#[test]
fn commits_ahead_returns_oldest_first() {
    let env = setup();
    let handle = env.mgr.create("m1", "ws1", "main").unwrap();
    let c1 = env.commit_file(&handle.path, "a.txt", "a\n", "first");
    let c2 = env.commit_file(&handle.path, "b.txt", "b\n", "second");

    let commits = env.mgr.commits_ahead("bridge/m1/ws1", "main").unwrap();
    assert_eq!(commits, vec![c1, c2]);

    // Nothing ahead of itself.
    assert!(env.mgr.commits_ahead("bridge/m1/ws1", "bridge/m1/ws1").unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// cherry_pick()
// ---------------------------------------------------------------------------

#[test]
fn cherry_pick_applies_commits_in_order() {
    let env = setup();
    let source = env.mgr.create("m1", "ws1", "main").unwrap();
    let c1 = env.commit_file(&source.path, "a1.txt", "a1\n", "add a1");
    let c2 = env.commit_file(&source.path, "a2.txt", "a2\n", "add a2");

    let target = env.mgr.create("m1", "ws2", "main").unwrap();
    let result = env.mgr.cherry_pick(&target, &[c1, c2]).unwrap();
    assert_eq!(result, Ok(()));
    assert!(target.path.join("a1.txt").is_file());
    assert!(target.path.join("a2.txt").is_file());
    let count = env.git(&target.path, &["rev-list", "--count", "main..HEAD"]);
    assert_eq!(count.trim(), "2");

    // Empty pick is a no-op success.
    assert_eq!(env.mgr.cherry_pick(&target, &[]).unwrap(), Ok(()));
}

#[test]
fn cherry_pick_conflict_aborts_and_reports_commit() {
    let env = setup();
    let source = env.mgr.create("m1", "ws1", "main").unwrap();
    let clean = env.commit_file(&source.path, "new.txt", "new\n", "clean commit");
    let conflicting =
        env.commit_file(&source.path, "seed.txt", "source version\n", "conflicting commit");

    let target = env.mgr.create("m1", "ws2", "main").unwrap();
    let pre = env.commit_file(&target.path, "seed.txt", "target version\n", "target seed edit");

    let result = env.mgr.cherry_pick(&target, &[clean.clone(), conflicting.clone()]).unwrap();
    assert_eq!(result, Err(conflicting));

    // The whole sequence was aborted: clean state, HEAD back at pre-pick,
    // the already-applied first commit rolled back too.
    assert_eq!(env.status_porcelain(&target.path), "");
    assert_eq!(env.head(&target.path), pre);
    assert!(!target.path.join("new.txt").exists());
    let _ = clean;
}

// ---------------------------------------------------------------------------
// list_bridge_worktrees()
// ---------------------------------------------------------------------------

#[test]
fn list_bridge_worktrees_returns_only_ours() {
    let env = setup();
    let h1 = env.mgr.create("m1", "ws1", "main").unwrap();
    let h2 = env.mgr.create("m1", "ws2", "main").unwrap();
    let throwaway = env.mgr.create_throwaway("bridge/m1/ws1").unwrap();

    // A foreign worktree outside our root must not be reported.
    let external = env.base.join("external");
    env.git(
        &env.repo,
        &["worktree", "add", "-q", "-b", "external-branch", external.to_str().unwrap(), "main"],
    );

    let mut listed = env.mgr.list_bridge_worktrees().unwrap();
    listed.sort();
    let mut expected = vec![h1.path.clone(), h2.path.clone(), throwaway.path.clone()];
    expected.sort();
    assert_eq!(listed, expected);
}

// ---------------------------------------------------------------------------
// read_only()
// ---------------------------------------------------------------------------

#[test]
fn read_only_rejects_mutating_subcommands() {
    let env = setup();
    for argv in [
        vec!["worktree", "list"],
        vec!["commit", "-m", "x"],
        vec!["push"],
        vec!["merge", "main"],
        vec!["rebase", "main"],
        vec!["reset", "--hard"],
        vec!["branch", "-D", "x"],
        vec!["checkout", "main"],
        vec!["gc"],
    ] {
        let err = env.mgr.read_only(&argv).unwrap_err();
        assert!(
            matches!(err, GitError::Invalid(_)),
            "expected Invalid for {argv:?}, got {err:?}"
        );
    }

    // Empty argv and global options are rejected too.
    assert!(env.mgr.read_only(&[]).is_err());
    assert!(env.mgr.read_only(&["-c", "x.y=z", "log"]).is_err());
    // Output-to-file flags on otherwise read-only subcommands are refused.
    assert!(env.mgr.read_only(&["log", "--output=somewhere"]).is_err());
    assert!(env.mgr.read_only(&["diff", "--output", "somewhere"]).is_err());
}

#[test]
fn read_only_runs_log_show_diff() {
    let env = setup();

    let log = env.mgr.read_only(&["log", "--oneline"]).unwrap();
    assert!(log.status.success());
    assert!(String::from_utf8_lossy(&log.stdout).contains("seed"));

    let show = env.mgr.read_only(&["show", "--stat", "HEAD"]).unwrap();
    assert!(show.status.success());
    assert!(String::from_utf8_lossy(&show.stdout).contains("seed.txt"));

    let diff = env.mgr.read_only(&["diff"]).unwrap();
    assert!(diff.status.success());
}

#[test]
fn read_only_propagates_git_failures_as_command_errors() {
    let env = setup();
    let err = env.mgr.read_only(&["log", "no-such-ref"]).unwrap_err();
    match err {
        GitError::Command { args, status, stderr } => {
            assert!(args.iter().any(|a| a == "no-such-ref"));
            assert_ne!(status, 0);
            assert!(!stderr.is_empty());
        }
        other => panic!("expected Command error, got {other:?}"),
    }
}
