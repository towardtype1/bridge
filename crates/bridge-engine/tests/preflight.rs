//! preflight() integration tests against the fake claude CLI.

mod common;

use bridge_core::ClaudeConfig;
use bridge_engine::{EngineError, preflight::preflight};
use common::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn config_for(binary: &Path) -> ClaudeConfig {
    ClaudeConfig {
        binary_path: binary.to_string_lossy().into_owned(),
        turn_timeout_secs: 30,
        ..ClaudeConfig::default()
    }
}

#[tokio::test]
async fn parses_version_without_probing_a_turn() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("success"));

    let report = preflight(&config_for(&script), false).await.unwrap();

    assert_eq!(report.version.to_string(), "2.1.201");
    assert_eq!(report.raw_version, "2.1.201 (Claude Code)");
    // 2.1.201 sits inside the default tested range 2.1.190 ..= 2.2.99.
    assert!(report.version_tested);
    assert!(
        report.available_tools.is_empty(),
        "probe_tools=false must skip the tool probe"
    );
}

#[tokio::test]
async fn probe_tools_reads_the_init_event_tool_list() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("success"));

    let report = preflight(&config_for(&script), true).await.unwrap();

    assert_eq!(
        report.available_tools,
        vec!["Task", "Bash", "Edit", "Read", "WebFetch"]
    );
}

#[tokio::test]
async fn version_outside_tested_range_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("success"));
    let mut config = config_for(&script);
    config.tested_version_min = "1.0.0".into();
    config.tested_version_max = "2.0.99".into();

    let report = preflight(&config, false).await.unwrap();
    assert!(!report.version_tested, "2.1.201 is newer than 2.0.99");
}

#[tokio::test]
async fn missing_binary_is_a_spawn_error() {
    let config = ClaudeConfig {
        binary_path: "/nonexistent/claude-for-bridge-tests".into(),
        ..ClaudeConfig::default()
    };
    let err = preflight(&config, false).await.unwrap_err();
    assert!(matches!(err, EngineError::Spawn(_)), "got {err:?}");
}

#[tokio::test]
async fn unparseable_version_output_is_a_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("weird_claude.sh");
    std::fs::write(&path, "#!/bin/sh\necho \"Warp Drive vNaN\"\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = preflight(&config_for(&path), false).await.unwrap_err();
    match err {
        EngineError::Spawn(msg) => assert!(msg.contains("version"), "message: {msg}"),
        other => panic!("expected Spawn error, got {other:?}"),
    }
}

#[tokio::test]
async fn failing_version_command_is_a_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken_claude.sh");
    std::fs::write(&path, "#!/bin/sh\necho \"dilithium exhausted\" >&2\nexit 7\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = preflight(&config_for(&path), false).await.unwrap_err();
    match err {
        EngineError::Spawn(msg) => {
            assert!(msg.contains("dilithium exhausted"), "message: {msg}");
        }
        other => panic!("expected Spawn error, got {other:?}"),
    }
}
