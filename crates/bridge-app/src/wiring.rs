//! Composition root: builds the full engine stack and the channel pair
//! the GUI talks to.

use bridge_core::{BridgeCommand, BridgeConfig, BridgeEvent};
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};

pub struct Wiring {
    pub events: broadcast::Sender<BridgeEvent>,
    pub commands: mpsc::Sender<BridgeCommand>,
    /// Join handle of the controller task (owned by the background runtime).
    pub controller: tokio::task::JoinHandle<()>,
    /// Runtime kept alive for the app's lifetime.
    pub runtime: tokio::runtime::Runtime,
}

/// Build everything per the boot order in main.rs. `repo` is the target
/// repository the mission works on.
pub fn build(repo: PathBuf, config: BridgeConfig) -> Result<Wiring, Box<dyn std::error::Error>> {
    todo!()
}

/// Resolve the per-repo data dir (worktrees root default, db path):
/// `<platform data dir>/bridge/<sanitized-repo-path>/`.
pub fn data_dir_for_repo(repo: &PathBuf) -> PathBuf {
    todo!()
}
