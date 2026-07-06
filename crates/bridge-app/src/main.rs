//! The Bridge GUI application.
//!
//! `bridge [--repo <path>] [--config <bridge.toml>] [--headless-smoke] [objective]`
//!
//! Boot order:
//! 1. tracing init; config load (default `bridge.toml` in --repo, else
//!    built-in defaults),
//! 2. open the Ship's Computer (per-repo db under the platform data dir),
//! 3. orphan sweep: kill recorded child pids, prune stale worktrees,
//! 4. engine preflight (version check -> CompatWarning event when outside
//!    the tested range; GUI shows the banner with "proceed anyway"),
//! 5. bind the control server; build PolicyEngine, EscalationBroker,
//!    ClaudeRunner, WorktreeManager, LiveDeps,
//! 6. spawn MissionController on a background tokio runtime,
//! 7. run the eframe app on the main thread (event bus in, commands out).
//!
//! `--headless-smoke` skips eframe and drives one scripted mini-mission
//! for CI/e2e verification (see tests + Phase C of the plan): it sends
//! StartMission with the positional objective, prints every event as a
//! JSON line on stdout, auto-approves escalations and merge proposals,
//! and exits when the mission reaches Complete (0) or Failed (1).

mod state;
mod ui;
mod wiring;

use bridge_core::{BridgeCommand, BridgeConfig, BridgeEvent, MissionState, UserDecision};
use state::AppState;
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};
use wiring::Wiring;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = parse_args(std::env::args().skip(1))?;
    let repo = match &args.repo {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    let config = load_config(&repo, args.config.as_deref())?;

    let wiring = wiring::build(repo, config)?;
    if args.headless_smoke {
        run_headless(wiring, args.objective)
    } else {
        run_gui(wiring)
    }
}

/// Parsed command line. Hand-rolled on purpose: three flags do not earn a
/// clap dependency.
#[derive(Debug, Default, PartialEq)]
struct CliArgs {
    repo: Option<PathBuf>,
    config: Option<PathBuf>,
    headless_smoke: bool,
    /// First positional argument: the mission objective (headless mode).
    objective: Option<String>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<CliArgs, String> {
    let mut parsed = CliArgs::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => {
                let value = args.next().ok_or("--repo requires a path argument")?;
                parsed.repo = Some(PathBuf::from(value));
            }
            "--config" => {
                let value = args.next().ok_or("--config requires a path argument")?;
                parsed.config = Some(PathBuf::from(value));
            }
            "--headless-smoke" => parsed.headless_smoke = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown flag: {other}"));
            }
            positional => {
                if parsed.objective.is_some() {
                    return Err(format!("unexpected extra argument: {positional}"));
                }
                parsed.objective = Some(positional.to_owned());
            }
        }
    }
    Ok(parsed)
}

/// Explicit --config path must load (error out otherwise). Without one,
/// `<repo>/bridge.toml` is used when present, else built-in defaults.
fn load_config(
    repo: &std::path::Path,
    explicit: Option<&std::path::Path>,
) -> Result<BridgeConfig, Box<dyn std::error::Error>> {
    if let Some(path) = explicit {
        return Ok(BridgeConfig::load(path)?);
    }
    let default_path = repo.join("bridge.toml");
    if default_path.exists() {
        return Ok(BridgeConfig::load(&default_path)?);
    }
    Ok(BridgeConfig::default())
}

// -- GUI mode -----------------------------------------------------------------

struct BridgeApp {
    state: AppState,
    events_rx: broadcast::Receiver<BridgeEvent>,
    commands: mpsc::Sender<BridgeCommand>,
    out_commands: Vec<BridgeCommand>,
    shutdown_sent: bool,
    /// Keeps the tokio runtime (and with it the controller) alive.
    _wiring: Wiring,
}

impl BridgeApp {
    fn new(mut wiring: Wiring) -> Self {
        let events_rx = wiring
            .bootstrap_rx
            .take()
            .expect("bootstrap receiver present on a fresh Wiring");
        let commands = wiring.commands.clone();
        Self {
            state: AppState::default(),
            events_rx,
            commands,
            out_commands: Vec::new(),
            shutdown_sent: false,
            _wiring: wiring,
        }
    }

    fn drain_events(&mut self) {
        loop {
            match self.events_rx.try_recv() {
                Ok(event) => self.state.apply(event),
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "GUI lagged behind the event bus");
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
    }

    fn flush_commands(&mut self) {
        for command in self.out_commands.drain(..) {
            if let Err(err) = self.commands.try_send(command) {
                tracing::warn!(error = %err, "command channel rejected a GUI command");
            }
        }
    }
}

impl Drop for BridgeApp {
    /// Give the controller a moment to honor Shutdown (persist state, kill
    /// children) before the runtime is torn down with the window.
    fn drop(&mut self) {
        if !self.shutdown_sent {
            let _ = self.commands.try_send(BridgeCommand::Shutdown);
        }
        let controller = &mut self._wiring.controller;
        let _ = self._wiring.runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), controller).await
        });
    }
}

impl eframe::App for BridgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        let ctx = ui.ctx().clone();
        ui::draw(&ctx, &mut self.state, &mut self.out_commands);
        self.flush_commands();
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.shutdown_sent {
            self.shutdown_sent = true;
            if let Err(err) = self.commands.try_send(BridgeCommand::Shutdown) {
                tracing::warn!(error = %err, "could not send Shutdown on window close");
            }
        }
    }
}

use ui::egui;

fn run_gui(wiring: Wiring) -> Result<(), Box<dyn std::error::Error>> {
    let events = wiring.events.clone();
    let runtime_handle = wiring.runtime.handle().clone();
    eframe::run_native(
        "Bridge",
        eframe::NativeOptions::default(),
        Box::new(move |cc| {
            wiring::spawn_repaint_forwarder(&runtime_handle, &events, Some(cc.egui_ctx.clone()));
            Ok(Box::new(BridgeApp::new(wiring)))
        }),
    )?;
    Ok(())
}

// -- headless smoke mode --------------------------------------------------------

/// Phase C harness: drive one scripted mission without a display.
fn run_headless(
    mut wiring: Wiring,
    objective: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let objective =
        objective.ok_or("--headless-smoke requires a mission objective argument")?;
    let mut events_rx = wiring
        .bootstrap_rx
        .take()
        .expect("bootstrap receiver present on a fresh Wiring");

    wiring
        .commands
        .blocking_send(BridgeCommand::StartMission { objective })?;

    let mut outcome: Result<(), Box<dyn std::error::Error>> = Ok(());
    loop {
        let event = match events_rx.blocking_recv() {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "headless driver lagged behind the event bus");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                outcome = Err("event bus closed before the mission finished".into());
                break;
            }
        };
        println!("{}", serde_json::to_string(&event)?);

        match &event {
            BridgeEvent::EscalationRequested(ticket) => {
                wiring.commands.blocking_send(BridgeCommand::ResolveEscalation {
                    id: ticket.id,
                    decision: UserDecision::Approve,
                })?;
            }
            BridgeEvent::MergeConfirmationRequested(proposal) => {
                wiring.commands.blocking_send(BridgeCommand::ConfirmMerge {
                    workstream: proposal.workstream,
                    approved: true,
                })?;
            }
            BridgeEvent::MissionStatus(update) => match &update.state {
                MissionState::Complete => break,
                MissionState::Failed { reason } => {
                    outcome = Err(format!("mission failed: {reason}").into());
                    break;
                }
                _ => {}
            },
            _ => {}
        }
    }

    let _ = wiring.commands.blocking_send(BridgeCommand::Shutdown);
    let _ = wiring.runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), wiring.controller).await
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> impl Iterator<Item = String> + use<> {
        parts
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parse_args_empty_is_all_defaults() {
        let args = parse_args(argv(&[])).unwrap();
        assert_eq!(args, CliArgs::default());
    }

    #[test]
    fn parse_args_full_set() {
        let args = parse_args(argv(&[
            "--repo",
            "/tmp/repo",
            "--config",
            "/tmp/bridge.toml",
            "--headless-smoke",
            "fix the warp core",
        ]))
        .unwrap();
        assert_eq!(args.repo, Some(PathBuf::from("/tmp/repo")));
        assert_eq!(args.config, Some(PathBuf::from("/tmp/bridge.toml")));
        assert!(args.headless_smoke);
        assert_eq!(args.objective.as_deref(), Some("fix the warp core"));
    }

    #[test]
    fn parse_args_objective_position_is_flexible() {
        let args = parse_args(argv(&["fix it", "--headless-smoke"])).unwrap();
        assert!(args.headless_smoke);
        assert_eq!(args.objective.as_deref(), Some("fix it"));
    }

    #[test]
    fn parse_args_missing_flag_value_errors() {
        assert!(parse_args(argv(&["--repo"])).is_err());
        assert!(parse_args(argv(&["--config"])).is_err());
    }

    #[test]
    fn parse_args_unknown_flag_errors() {
        let err = parse_args(argv(&["--warp-speed"])).unwrap_err();
        assert!(err.contains("--warp-speed"));
    }

    #[test]
    fn parse_args_second_positional_errors() {
        assert!(parse_args(argv(&["one", "two"])).is_err());
    }

    #[test]
    fn load_config_defaults_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load_config(dir.path(), None).unwrap();
        assert_eq!(cfg, BridgeConfig::default());
    }

    #[test]
    fn load_config_picks_up_repo_bridge_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bridge.toml"), "[budgets]\nmax_total_turns = 9\n")
            .unwrap();
        let cfg = load_config(dir.path(), None).unwrap();
        assert_eq!(cfg.budgets.max_total_turns, 9);
    }

    #[test]
    fn load_config_explicit_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert!(load_config(dir.path(), Some(&missing)).is_err());
    }
}
