//! Composition root: builds the full engine stack and the channel pair
//! the GUI talks to.

use bridge_core::{BridgeCommand, BridgeConfig, BridgeEvent, LogEntry, LogLevel};
use bridge_computer::ShipsComputer;
use bridge_engine::ClaudeRunner;
use bridge_git::WorktreeManager;
use bridge_stations::{LiveDeps, MissionController};
use bridge_tactical::{ControlServer, EscalationBroker, PolicyEngine};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

pub struct Wiring {
    pub events: broadcast::Sender<BridgeEvent>,
    pub commands: mpsc::Sender<BridgeCommand>,
    /// Join handle of the controller task (owned by the background runtime).
    pub controller: tokio::task::JoinHandle<()>,
    /// Runtime kept alive for the app's lifetime.
    pub runtime: tokio::runtime::Runtime,
    /// The channel's original receiver, kept so events emitted during boot
    /// (e.g. the preflight CompatWarning) are retained and delivered to the
    /// first consumer. Take it instead of calling `events.subscribe()`.
    pub bootstrap_rx: Option<broadcast::Receiver<BridgeEvent>>,
}

/// Build everything per the boot order in main.rs. `repo` is the target
/// repository the mission works on.
pub fn build(repo: PathBuf, config: BridgeConfig) -> Result<Wiring, Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let (events, bootstrap_rx) = broadcast::channel::<BridgeEvent>(4096);
    let (commands_tx, commands_rx) = mpsc::channel::<BridgeCommand>(256);

    // 2. Ship's Computer under the per-repo data dir.
    let data_dir = data_dir_for_repo(&repo);
    std::fs::create_dir_all(&data_dir)?;
    let computer = Arc::new(ShipsComputer::open(&data_dir.join("bridge.sqlite3"))?);

    // 3. Orphan sweep: SIGTERM pids recorded by a previous run, best-effort.
    sweep_orphan_pids(&computer);

    // 5a. Worktree manager (root outside the target repo).
    let worktrees_root = config
        .worktrees
        .root
        .clone()
        .unwrap_or_else(|| data_dir.join("worktrees"));
    std::fs::create_dir_all(&worktrees_root)?;
    let worktrees = Arc::new(WorktreeManager::new(repo, worktrees_root)?);

    // 5b. Tactical: policy engine, escalation broker, control server.
    let policy = Arc::new(PolicyEngine::new(config.tactical.clone())?);
    if config.tactical.red_alert_default {
        policy.set_red_alert(true);
    }
    let broker = Arc::new(EscalationBroker::new());
    let server = ControlServer::new(
        Arc::clone(&policy),
        Arc::clone(&broker),
        events.clone(),
        config.tactical.escalation_timeout_secs,
        None,
    );
    let server_handle = Arc::new(runtime.block_on(server.bind())?);

    // 4. Preflight: CompatWarning event when outside the tested range.
    match runtime.block_on(bridge_engine::preflight::preflight(&config.claude, false)) {
        Ok(report) => {
            if !report.version_tested {
                let _ = events.send(BridgeEvent::CompatWarning {
                    detected: report.version.to_string(),
                    tested_min: config.claude.tested_version_min.clone(),
                    tested_max: config.claude.tested_version_max.clone(),
                });
            }
        }
        Err(err) => {
            let _ = events.send(BridgeEvent::Log(LogEntry::new(
                LogLevel::Warn,
                format!("preflight failed: {err}"),
            )));
        }
    }

    // 5c. Engine + live port wiring.
    let runner = Arc::new(ClaudeRunner::new(config.claude.clone(), events.clone()));
    let deps = Arc::new(LiveDeps {
        runner,
        worktrees,
        policy,
        server: server_handle,
        computer,
        broker,
    });

    // 6. Mission controller on the background runtime.
    let helper_path = hook_helper_path();
    let controller = MissionController::new(deps, config, helper_path, events.clone(), commands_rx);
    let controller_handle = runtime.spawn(async move {
        if let Err(err) = controller.run().await {
            tracing::error!(error = %err, "mission controller exited with error");
        }
    });

    Ok(Wiring {
        events,
        commands: commands_tx,
        controller: controller_handle,
        runtime,
        bootstrap_rx: Some(bootstrap_rx),
    })
}

/// Spawn the event-forwarder task: on every bus event, ask egui for a
/// repaint so the GUI thread wakes and drains its receiver. `ctx` is None
/// in headless mode, where the task only keeps the bus drained.
pub fn spawn_repaint_forwarder(
    handle: &tokio::runtime::Handle,
    events: &broadcast::Sender<BridgeEvent>,
    ctx: Option<eframe::egui::Context>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = events.subscribe();
    handle.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(_) => {
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "repaint forwarder lagged behind the event bus");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Resolve the per-repo data dir (worktrees root default, db path):
/// `~/.bridge/<sanitized-repo-path>/`.
///
/// Deliberately NOT the platform data dir: on macOS that is
/// `~/Library/Application Support`, and a space inside every worktree
/// path degrades anything that round-trips paths through shell strings
/// (agent Bash commands, hook settings, policy heuristics). A dot-dir in
/// $HOME is space-free on every platform.
// The `&PathBuf` parameter is a frozen contract signature; `&Path` would
// be idiomatic (clippy::ptr_arg) but changing it is not allowed here.
#[allow(clippy::ptr_arg)]
pub fn data_dir_for_repo(repo: &PathBuf) -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".bridge").join(sanitize_repo_path(repo))
}

/// Flatten a repo path into a single directory name: every path separator
/// becomes '-', with no leading or trailing dashes.
fn sanitize_repo_path(repo: &std::path::Path) -> String {
    repo.to_string_lossy()
        .replace(['/', '\\'], "-")
        .trim_matches('-')
        .to_owned()
}

/// SIGTERM every child pid recorded by a previous run, then clear them.
/// Best-effort: a dead pid or a failed kill is only logged.
fn sweep_orphan_pids(computer: &ShipsComputer) {
    let pids = match computer.recorded_child_pids() {
        Ok(pids) => pids,
        Err(err) => {
            tracing::warn!(error = %err, "could not read recorded child pids");
            return;
        }
    };
    for pid in pids {
        let outcome = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        match outcome {
            Ok(status) if status.success() => {
                tracing::info!(pid, "terminated orphan claude child");
            }
            Ok(_) => tracing::debug!(pid, "orphan pid already gone"),
            Err(err) => tracing::warn!(pid, error = %err, "failed to signal orphan pid"),
        }
        if let Err(err) = computer.clear_child_pid(pid) {
            tracing::warn!(pid, error = %err, "failed to clear recorded pid");
        }
    }
}

/// The hook helper binary is installed next to the app binary.
fn hook_helper_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("bridge-hook-helper")))
        .unwrap_or_else(|| PathBuf::from("bridge-hook-helper"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_a_space_free_home_dot_dir() {
        let repo = PathBuf::from("/Users/kirk/code/enterprise");
        let dir = data_dir_for_repo(&repo);
        let expected_base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".bridge");
        assert!(
            dir.starts_with(&expected_base),
            "{dir:?} should start with {expected_base:?}"
        );
        assert_eq!(
            dir.file_name().unwrap().to_string_lossy(),
            "Users-kirk-code-enterprise"
        );
        assert!(
            !dir.strip_prefix(dirs::home_dir().unwrap_or_default())
                .unwrap_or(&dir)
                .to_string_lossy()
                .contains(' '),
            "harness-managed paths must stay space-free: {dir:?}"
        );
    }

    #[test]
    fn sanitize_replaces_separators_and_trims_dashes() {
        assert_eq!(
            sanitize_repo_path(&PathBuf::from("/a/b/c")),
            "a-b-c"
        );
        assert_eq!(
            sanitize_repo_path(&PathBuf::from("relative/repo")),
            "relative-repo"
        );
        assert_eq!(sanitize_repo_path(&PathBuf::from("plain")), "plain");
    }

    #[test]
    fn distinct_repos_get_distinct_dirs() {
        let a = data_dir_for_repo(&PathBuf::from("/repo/one"));
        let b = data_dir_for_repo(&PathBuf::from("/repo/two"));
        assert_ne!(a, b);
    }
}
