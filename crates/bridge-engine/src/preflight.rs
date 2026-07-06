//! Startup preflight: version, auth mode, tool inventory.

use bridge_compat::{
    ClaudeInvocation, CliVersion, StreamEvent, VersionStatus, check_version, parse_line,
    scrubbed_env,
};
use bridge_core::ClaudeConfig;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::runner::EngineError;

/// `claude --version` is a local call; anything slower than this is stuck.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct PreflightReport {
    pub version: CliVersion,
    pub raw_version: String,
    /// From `bridge_compat::check_version`.
    pub version_tested: bool,
    /// Tool names the CLI reports (from a probe run's system/init event),
    /// used to validate station allowlists. Empty when the probe was
    /// skipped.
    pub available_tools: Vec<String>,
}

/// Run `claude --version` (cheap, no model call) and parse it. Does NOT
/// probe a full turn by default; `probe_tools` additionally runs a
/// max-turns-1 haiku no-op to capture the init event's tool list.
pub async fn preflight(
    config: &ClaudeConfig,
    probe_tools: bool,
) -> Result<PreflightReport, EngineError> {
    let mut cmd = Command::new(&config.binary_path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for var in scrubbed_env() {
        cmd.env_remove(var);
    }
    let output = tokio::time::timeout(VERSION_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            EngineError::Spawn(format!(
                "claude --version timed out after {}s",
                VERSION_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|err| {
            EngineError::Spawn(format!(
                "failed to run {} --version: {err}",
                config.binary_path
            ))
        })?;
    if !output.status.success() {
        return Err(EngineError::Spawn(format!(
            "claude --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let raw_version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let version: CliVersion = raw_version.parse().map_err(|err| {
        EngineError::Spawn(format!("cannot parse claude --version output: {err}"))
    })?;
    let version_tested = check_version(&version, config) == VersionStatus::Tested;
    if !version_tested {
        tracing::warn!(
            detected = %version,
            tested_min = %config.tested_version_min,
            tested_max = %config.tested_version_max,
            "claude CLI version is outside the tested range"
        );
    }

    let available_tools = if probe_tools {
        probe_tool_list(config).await?
    } else {
        Vec::new()
    };

    Ok(PreflightReport {
        version,
        raw_version,
        version_tested,
        available_tools,
    })
}

/// Spawn a minimal one-turn run and read the `system/init` event's tool
/// list. The child is killed as soon as the init event is captured.
async fn probe_tool_list(config: &ClaudeConfig) -> Result<Vec<String>, EngineError> {
    let inv = ClaudeInvocation {
        prompt: "Preflight probe. Reply with exactly: OK".into(),
        cwd: std::env::temp_dir(),
        max_turns: 1,
        model: Some("haiku".into()),
        ..Default::default()
    };
    let mut cmd = Command::new(&config.binary_path);
    cmd.args(inv.to_args())
        .current_dir(&inv.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in scrubbed_env() {
        cmd.env_remove(var);
    }
    let mut child = cmd
        .spawn()
        .map_err(|err| EngineError::Spawn(format!("failed to spawn probe run: {err}")))?;
    let stdout = child.stdout.take().expect("stdout was piped");

    let timeout = Duration::from_secs(config.turn_timeout_secs.max(1));
    let init_tools = tokio::time::timeout(timeout, async {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(StreamEvent::SystemInit(init)) = parse_line(&line) {
                return Some(init.tools);
            }
        }
        None
    })
    .await;

    // The init event is all we need; do not burn the rest of the turn.
    let _ = child.kill().await;
    let _ = child.wait().await;

    match init_tools {
        Ok(Some(tools)) => Ok(tools),
        Ok(None) => Err(EngineError::Spawn(
            "probe run ended without a system/init event".into(),
        )),
        Err(_) => Err(EngineError::Spawn(
            "probe run timed out before emitting system/init".into(),
        )),
    }
}
