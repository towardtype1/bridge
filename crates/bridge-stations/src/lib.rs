//! Stations: agent role definitions and the mission orchestration built
//! on top of the engine, git, tactical and computer crates.
//!
//! Only Captain, Helm, Science, Kobayashi Maru and Comms spawn claude
//! agents. Ops, Engineering and Tactical are procedural stations whose
//! work is harness code (scheduling, worktree lifecycle, policy); they
//! appear in logs and turn accounting but never hold a model session.

pub mod controller;
pub mod engineering;
pub mod kobayashi;
pub mod merge_queue;
pub mod ports;
pub mod profiles;
pub mod prompts;

pub use controller::{MissionController, MissionError};
pub use kobayashi::KobayashiRunner;
pub use merge_queue::MergeQueue;
pub use ports::{ComputerPort, GitPort, LiveDeps, TacticalPort, TurnPort};
pub use profiles::{StationProfile, station_profile};
