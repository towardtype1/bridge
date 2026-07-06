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
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The only directory the Kobayashi Maru may write under, relative to its
/// worktree root. Enforced by Tactical path policy; propagated at
/// provisioning time.
pub const ADVERSARIAL_DIR: &str = "tests/adversarial";

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

/// Render and write `<worktree>/.claude/settings.json` (0600) with the
/// hook wiring for this workstream.
fn install_settings(
    worktree: &Path,
    server_url: &str,
    ws: WorkstreamId,
    token: &str,
    helper_path: &Path,
    hook_timeout_secs: u32,
) -> Result<PathBuf, std::io::Error> {
    let dir = worktree.join(".claude");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("settings.json");
    let value =
        bridge_compat::render_worktree_settings(server_url, ws, token, helper_path, hook_timeout_secs);
    let body = serde_json::to_vec_pretty(&value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&path)?;
    file.write_all(&body)?;
    // mode() only applies on creation; force 0600 even if the file existed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

fn provision_handle<G: GitPort, T: TacticalPort>(
    git: &G,
    tactical: &T,
    claude_cfg: &ClaudeConfig,
    helper_path: &Path,
    ws: WorkstreamId,
    handle: WorktreeHandle,
    write_only_under: Option<PathBuf>,
) -> Result<ProvisionedWorktree, EngineeringError> {
    let ctx = WorkstreamCtx {
        worktree_path: handle.path.clone(),
        write_only_under,
    };
    let (server_url, token) = tactical.arm_workstream(ws, ctx);
    let settings_path = install_settings(
        &handle.path,
        &server_url,
        ws,
        &token,
        helper_path,
        claude_cfg.hook_timeout_secs,
    )?;
    git.add_worktree_exclude(&handle, &[".claude/"])?;
    Ok(ProvisionedWorktree {
        handle,
        settings_path,
    })
}

/// Create the worktree for (mission, workstream), arm Tactical, render
/// `.claude/settings.json` via `bridge_compat::render_worktree_settings`
/// with the issued token, write it 0600, and git-exclude `.claude/`.
/// `write_only_under` propagates the Kobayashi path restriction.
#[allow(clippy::too_many_arguments)]
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
    let handle = git.create_worktree(mission_slug, ws_slug, base_ref)?;
    provision_handle(git, tactical, claude_cfg, helper_path, ws, handle, write_only_under)
}

/// Same, but a throwaway worktree at `branch`'s head for one Kobayashi
/// round (fresh identity: new token, its own settings file). Writes are
/// restricted to `tests/adversarial/`.
pub fn provision_throwaway<G: GitPort, T: TacticalPort>(
    git: &G,
    tactical: &T,
    claude_cfg: &ClaudeConfig,
    helper_path: &Path,
    ws: WorkstreamId,
    branch: &str,
) -> Result<ProvisionedWorktree, EngineeringError> {
    let handle = git.create_throwaway(branch)?;
    provision_handle(
        git,
        tactical,
        claude_cfg,
        helper_path,
        ws,
        handle,
        Some(PathBuf::from(ADVERSARIAL_DIR)),
    )
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
    tactical.disarm_workstream(ws);
    if let Err(e) = std::fs::remove_file(&provisioned.settings_path) {
        tracing::warn!(
            "failed to remove settings file {}: {e}",
            provisioned.settings_path.display()
        );
    }
    if let Err(e) = git.remove_worktree(&provisioned.handle, !keep) {
        tracing::warn!(
            "failed to remove worktree {}: {e}",
            provisioned.handle.path.display()
        );
    }
}

/// Locate the bundled helper binary: next to the current executable
/// (release layout) or under CARGO target dir (dev). Errors if missing so
/// missions cannot start without the veto path.
pub fn locate_helper() -> Result<PathBuf, EngineeringError> {
    let exe = std::env::current_exe().map_err(EngineeringError::Install)?;
    let name = if cfg!(windows) {
        "bridge-hook-helper.exe"
    } else {
        "bridge-hook-helper"
    };
    let mut candidates = Vec::new();
    if let Some(dir) = exe.parent() {
        // Release layout: helper installed next to the app binary.
        candidates.push(dir.join(name));
        // Dev layout: test binaries live in target/<profile>/deps; built
        // binaries land one level up in target/<profile>.
        candidates.push(dir.join("..").join(name));
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.canonicalize().unwrap_or_else(|_| candidate.clone()));
        }
    }
    Err(EngineeringError::Install(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("bridge-hook-helper not found; looked at {candidates:?}"),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{GitCall, MockDeps};

    fn helper() -> PathBuf {
        PathBuf::from("/opt/bridge/bridge-hook-helper")
    }

    #[test]
    fn provision_installs_settings_and_excludes_claude_dir() {
        let deps = MockDeps::new();
        let cfg = ClaudeConfig::default();
        let ws = WorkstreamId::new();

        let p = provision(
            &*deps, &*deps, &cfg, &helper(), "mission-x", ws, "ws-a", "main", None,
        )
        .expect("provision");

        // Worktree created from the right base.
        assert!(deps.git_log().contains(&GitCall::CreateWorktree {
            mission: "mission-x".into(),
            ws: "ws-a".into(),
            base: "main".into(),
        }));
        assert_eq!(p.handle.path, deps.root.path().join("mission-x").join("ws-a"));

        // Settings file written at <worktree>/.claude/settings.json ...
        let expected_path = p.handle.path.join(".claude").join("settings.json");
        assert_eq!(p.settings_path, expected_path);
        let raw = std::fs::read_to_string(&expected_path).expect("settings readable");

        // ... with 0600 permissions ...
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&expected_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "settings must be 0600");
        }

        // ... whose content is exactly the compat rendering with the token
        // issued by arm_workstream.
        let armed = deps.armed.lock().unwrap();
        let (armed_ws, armed_ctx, token) = &armed[0];
        assert_eq!(*armed_ws, ws);
        assert_eq!(armed_ctx.worktree_path, p.handle.path);
        assert_eq!(armed_ctx.write_only_under, None);
        let expected = bridge_compat::render_worktree_settings(
            "http://127.0.0.1:45045",
            ws,
            token,
            &helper(),
            cfg.hook_timeout_secs,
        );
        let actual: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(actual, expected);
        assert!(raw.contains(token), "token must be embedded in the settings");

        // .claude/ excluded from git status via the worktree exclude file.
        assert!(deps.git_log().iter().any(|c| matches!(
            c,
            GitCall::Exclude { patterns, .. } if patterns == &vec![".claude/".to_string()]
        )));
    }

    #[test]
    fn provision_throwaway_restricts_writes_to_adversarial_dir() {
        let deps = MockDeps::new();
        let cfg = ClaudeConfig::default();
        let ws = WorkstreamId::new();

        let p = provision_throwaway(&*deps, &*deps, &cfg, &helper(), ws, "bridge/m/ws-a")
            .expect("provision throwaway");

        assert!(deps
            .git_log()
            .contains(&GitCall::CreateThrowaway { branch: "bridge/m/ws-a".into() }));
        assert!(p.handle.path.starts_with(deps.root.path().join("throwaway")));
        assert!(p.settings_path.is_file());

        let armed = deps.armed.lock().unwrap();
        let (_, ctx, _) = &armed[0];
        assert_eq!(ctx.worktree_path, p.handle.path);
        assert_eq!(ctx.write_only_under, Some(PathBuf::from("tests/adversarial")));
    }

    #[test]
    fn each_provision_gets_a_fresh_token() {
        let deps = MockDeps::new();
        let cfg = ClaudeConfig::default();
        let ws = WorkstreamId::new();
        provision_throwaway(&*deps, &*deps, &cfg, &helper(), ws, "b").unwrap();
        provision_throwaway(&*deps, &*deps, &cfg, &helper(), ws, "b").unwrap();
        let armed = deps.armed.lock().unwrap();
        assert_ne!(armed[0].2, armed[1].2, "throwaway rounds must get fresh tokens");
    }

    #[test]
    fn decommission_disarms_removes_settings_and_worktree() {
        let deps = MockDeps::new();
        let cfg = ClaudeConfig::default();
        let ws = WorkstreamId::new();
        let p = provision(
            &*deps, &*deps, &cfg, &helper(), "mission-x", ws, "ws-a", "main", None,
        )
        .unwrap();
        assert!(p.settings_path.is_file());

        decommission(&*deps, &*deps, ws, &p, false);

        assert_eq!(deps.disarmed.lock().unwrap().as_slice(), &[ws]);
        assert!(!p.settings_path.exists(), "settings file must be deleted");
        assert!(deps.git_log().contains(&GitCall::RemoveWorktree {
            path: p.handle.path.clone(),
            force: true,
        }));
    }

    #[test]
    fn decommission_keep_does_not_force_and_swallows_errors() {
        let deps = MockDeps::new();
        let cfg = ClaudeConfig::default();
        let ws = WorkstreamId::new();
        let p = provision(
            &*deps, &*deps, &cfg, &helper(), "mission-x", ws, "ws-a", "main", None,
        )
        .unwrap();
        // Delete the settings file up front and make worktree removal fail:
        // decommission must not panic or propagate.
        std::fs::remove_file(&p.settings_path).unwrap();
        *deps.fail_remove_worktree.lock().unwrap() = true;

        decommission(&*deps, &*deps, ws, &p, true);

        assert_eq!(deps.disarmed.lock().unwrap().as_slice(), &[ws]);
        assert!(deps.git_log().contains(&GitCall::RemoveWorktree {
            path: p.handle.path.clone(),
            force: false,
        }));
    }

    #[test]
    fn locate_helper_finds_dev_layout_and_errors_when_missing() {
        // Test binaries live in target/<dir>/debug/deps; the dev candidate
        // is target/<dir>/debug/bridge-hook-helper.
        let exe = std::env::current_exe().unwrap();
        let deps_dir = exe.parent().unwrap().to_owned();
        let sibling = deps_dir.join("bridge-hook-helper");
        let dev = deps_dir.join("..").join("bridge-hook-helper");

        // Clean slate: neither candidate present -> error.
        let _ = std::fs::remove_file(&sibling);
        let _ = std::fs::remove_file(&dev);
        assert!(matches!(locate_helper(), Err(EngineeringError::Install(_))));

        // Fake helper in the dev location -> found and canonicalized.
        std::fs::write(&dev, b"#!/bin/sh\n").unwrap();
        let found = locate_helper().expect("helper located");
        assert_eq!(found, dev.canonicalize().unwrap());
        let _ = std::fs::remove_file(&dev);
    }
}
