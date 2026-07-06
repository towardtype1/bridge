//! run_turn integration tests against the fake claude CLI.

mod common;

use bridge_core::{BridgeEvent, LogLevel, RateLimitState, SessionId, Station};
use bridge_engine::{ClaudeRunner, ExitClass};
use chrono::DateTime;
use common::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn success_turn_populates_outcome_and_emits_events() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("success"));
    let (runner, mut rx) = runner(&script, 3, 1, 30);
    let ctx = turn_ctx(false);

    let outcome = runner
        .run_turn(invocation(dir.path(), "make it so"), ctx.clone())
        .await
        .unwrap();

    assert_eq!(outcome.exit, ExitClass::Success);
    let result = outcome.result.expect("result event collected");
    assert_eq!(result.subtype, "success");
    assert!(!result.is_error);
    assert_eq!(result.session_id.as_deref(), Some("fake-session-1"));
    assert_eq!(result.num_turns, Some(2));
    assert_eq!(result.result.as_deref(), Some("DONE"));
    assert!(outcome.rate_limit.is_none());
    assert!(outcome.structured_output.is_none());

    let events = drain_events(&mut rx);
    let agent_output = events
        .iter()
        .find_map(|event| match event {
            BridgeEvent::AgentOutput {
                workstream,
                station,
                text,
            } => Some((workstream, station, text)),
            _ => None,
        })
        .expect("an AgentOutput event was observed on the bus");
    assert_eq!(*agent_output.0, ctx.workstream);
    assert_eq!(*agent_output.1, Station::Helm);
    assert_eq!(agent_output.2, "Engage.");

    let tool_call = events
        .iter()
        .find_map(|event| match event {
            BridgeEvent::ToolCall {
                tool_name, summary, ..
            } => Some((tool_name, summary)),
            _ => None,
        })
        .expect("a ToolCall event was observed on the bus");
    assert_eq!(tool_call.0, "Bash");
    assert!(
        tool_call.1.contains("echo LCARS-OK"),
        "summary: {}",
        tool_call.1
    );

    // The malformed and unknown-type lines in the stream were skipped
    // without failing the turn; the malformed one produced a warn log.
    assert!(events.iter().any(|event| matches!(
        event,
        BridgeEvent::Log(entry)
            if entry.level == LogLevel::Warn && entry.message.contains("malformed")
    )));
}

#[tokio::test]
async fn nonzero_exit_is_classified_and_stderr_captured_in_log_event() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("nonzero"));
    let (runner, mut rx) = runner(&script, 3, 1, 30);

    let outcome = runner
        .run_turn(invocation(dir.path(), "fail please"), turn_ctx(false))
        .await
        .unwrap();

    assert_eq!(outcome.exit, ExitClass::NonZero(3));
    let events = drain_events(&mut rx);
    assert!(
        events.iter().any(|event| matches!(
            event,
            BridgeEvent::Log(entry)
                if entry.level == LogLevel::Error
                    && entry.message.contains("warp core breach")
        )),
        "stderr should be captured into an error Log event, got {events:?}"
    );
}

#[tokio::test]
async fn timeout_kills_the_child_process() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("hang"));
    let (runner, _rx) = runner(&script, 3, 1, 2);

    // Capture the child pid through the runner's own registration callback
    // (robust against slow script startup under parallel test load).
    let pids: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let pids_in_cb = Arc::clone(&pids);
    let mut ctx = turn_ctx(false);
    ctx.pid_register = Some(Arc::new(move |pid, alive| {
        if alive {
            pids_in_cb.lock().unwrap().push(pid);
        }
    }));

    let started = std::time::Instant::now();
    let outcome = runner
        .run_turn(invocation(dir.path(), "hang forever"), ctx)
        .await
        .unwrap();

    assert_eq!(outcome.exit, ExitClass::TimedOut);
    assert!(outcome.result.is_none());
    // TERM should be honored by the shell well inside the 5s grace window.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(9),
        "kill took {:?}",
        started.elapsed()
    );
    let pid = pids
        .lock()
        .unwrap()
        .first()
        .copied()
        .expect("pid registered");
    assert!(
        !process_alive(pid),
        "fake claude pid {pid} is still alive after timeout kill"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn general_turns_respect_the_general_slot_count() {
    let dir = tempfile::tempdir().unwrap();
    let overlap = dir.path().join("overlap.log");
    let script = write_fake_claude(
        dir.path(),
        &FakeOpts {
            behavior: "success",
            delay_ms: 300,
            overlap_log: Some(overlap.clone()),
            ..FakeOpts::default()
        },
    );
    // max_concurrent=2 with 1 reserved Kobayashi slot leaves ONE general slot.
    let (runner, _rx) = runner(&script, 2, 1, 30);

    let (a, b, c) = tokio::join!(
        runner.run_turn(invocation(dir.path(), "general one"), turn_ctx(false)),
        runner.run_turn(invocation(dir.path(), "general two"), turn_ctx(false)),
        runner.run_turn(invocation(dir.path(), "general three"), turn_ctx(false)),
    );
    assert_eq!(a.unwrap().exit, ExitClass::Success);
    assert_eq!(b.unwrap().exit, ExitClass::Success);
    assert_eq!(c.unwrap().exit, ExitClass::Success);

    let log = std::fs::read_to_string(&overlap).unwrap();
    assert_eq!(
        max_overlap(&log, None),
        1,
        "three general turns must serialize through the single general slot:\n{log}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kobayashi_takes_the_reserved_slot_while_generals_queue() {
    let dir = tempfile::tempdir().unwrap();
    let overlap = dir.path().join("overlap.log");
    let script = write_fake_claude(
        dir.path(),
        &FakeOpts {
            behavior: "success",
            delay_ms: 800,
            overlap_log: Some(overlap.clone()),
            ..FakeOpts::default()
        },
    );
    let (runner, _rx) = runner(&script, 2, 1, 30);

    let (a, b, k) = tokio::join!(
        runner.run_turn(invocation(dir.path(), "general alpha"), turn_ctx(false)),
        runner.run_turn(invocation(dir.path(), "general beta"), turn_ctx(false)),
        runner.run_turn(
            invocation(dir.path(), "KOBAYASHI adversarial probe"),
            turn_ctx(true)
        ),
    );
    assert_eq!(a.unwrap().exit, ExitClass::Success);
    assert_eq!(b.unwrap().exit, ExitClass::Success);
    assert_eq!(k.unwrap().exit, ExitClass::Success);

    let log = std::fs::read_to_string(&overlap).unwrap();
    assert_eq!(
        max_overlap(&log, Some("general")),
        1,
        "generals may never exceed the general slot count:\n{log}"
    );
    assert_eq!(
        max_overlap(&log, None),
        2,
        "the kobayashi turn should run on the reserved slot concurrently \
         with a general turn while the other general queues:\n{log}"
    );
}

#[tokio::test]
async fn scrubbed_env_vars_are_removed_but_path_survives() {
    // SAFETY: test-only process-global env mutation; the fake script does
    // not read these variables and no other test asserts their absence.
    unsafe {
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-fake-for-test");
        std::env::set_var("CLAUDECODE", "1");
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dump.txt");
    let script = write_fake_claude(
        dir.path(),
        &FakeOpts {
            behavior: "success",
            out: Some(out.clone()),
            ..FakeOpts::default()
        },
    );
    let (runner, _rx) = runner(&script, 3, 1, 30);

    let outcome = runner
        .run_turn(invocation(dir.path(), "check env"), turn_ctx(false))
        .await
        .unwrap();
    assert_eq!(outcome.exit, ExitClass::Success);

    let dump = read_dump(&out);
    assert!(
        !dump.env.contains_key("ANTHROPIC_API_KEY"),
        "ANTHROPIC_API_KEY must be scrubbed from the child env"
    );
    assert!(
        !dump.env.contains_key("CLAUDECODE"),
        "CLAUDECODE must be scrubbed from the child env"
    );
    assert!(
        dump.env.contains_key("PATH"),
        "PATH must survive the scrub, env: {:?}",
        dump.env.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn resume_session_id_is_passed_through_to_argv() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dump.txt");
    let script = write_fake_claude(
        dir.path(),
        &FakeOpts {
            behavior: "success",
            out: Some(out.clone()),
            ..FakeOpts::default()
        },
    );
    let (runner, _rx) = runner(&script, 3, 1, 30);

    let mut inv = invocation(dir.path(), "resume me");
    inv.resume = Some(SessionId::from("resume-me-123"));
    let outcome = runner.run_turn(inv, turn_ctx(false)).await.unwrap();
    assert_eq!(outcome.exit, ExitClass::Success);

    let dump = read_dump(&out);
    let resume_pos = dump
        .args
        .iter()
        .position(|arg| arg == "--resume")
        .unwrap_or_else(|| panic!("--resume missing from argv: {:?}", dump.args));
    assert_eq!(
        dump.args.get(resume_pos + 1).map(String::as_str),
        Some("resume-me-123")
    );
    // Sanity: the headless flags rendered by bridge-compat are present.
    assert_eq!(dump.args.first().map(String::as_str), Some("-p"));
    assert!(dump.args.iter().any(|arg| arg == "stream-json"));
    assert!(!dump.args.iter().any(|arg| arg == "--bare"));
}

#[tokio::test]
async fn pid_register_is_called_with_true_then_false() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dump.txt");
    let script = write_fake_claude(
        dir.path(),
        &FakeOpts {
            behavior: "success",
            out: Some(out.clone()),
            ..FakeOpts::default()
        },
    );
    let (runner, _rx) = runner(&script, 3, 1, 30);

    let calls: Arc<Mutex<Vec<(u32, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_in_cb = Arc::clone(&calls);
    let mut ctx = turn_ctx(false);
    ctx.pid_register = Some(Arc::new(move |pid, alive| {
        calls_in_cb.lock().unwrap().push((pid, alive));
    }));

    let outcome = runner
        .run_turn(invocation(dir.path(), "track my pid"), ctx)
        .await
        .unwrap();
    assert_eq!(outcome.exit, ExitClass::Success);

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "expected register + clear, got {calls:?}");
    assert!(calls[0].1, "first call registers the pid");
    assert!(!calls[1].1, "second call clears the pid");
    assert_eq!(calls[0].0, calls[1].0, "same pid for register and clear");
    let dump = read_dump(&out);
    assert_eq!(
        calls[0].0, dump.pid,
        "registered pid is the child's real pid"
    );
}

#[tokio::test]
async fn rejected_rate_limit_event_is_detected_and_broadcast() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("rate_limited"));
    let (runner, mut rx) = runner(&script, 3, 1, 30);

    let outcome = runner
        .run_turn(invocation(dir.path(), "trigger limit"), turn_ctx(false))
        .await
        .unwrap();

    assert_eq!(outcome.exit, ExitClass::NonZero(1));
    let hit = outcome.rate_limit.expect("rate limit hit detected");
    assert_eq!(hit.retry_at, DateTime::from_timestamp(1785542400, 0));
    assert!(hit.trigger.contains("rejected"), "trigger: {}", hit.trigger);
    let result = outcome.result.expect("error result still collected");
    assert!(result.is_error);

    let events = drain_events(&mut rx);
    assert!(
        events.iter().any(|event| matches!(
            event,
            BridgeEvent::RateLimit(RateLimitState::Hit { retry_at })
                if *retry_at == DateTime::from_timestamp(1785542400, 0)
        )),
        "a RateLimit Hit event should reach the bus, got {events:?}"
    );
}

#[tokio::test]
async fn spawn_failure_is_classified_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    let config = bridge_core::ClaudeConfig {
        binary_path: "/nonexistent/claude-for-bridge-tests".into(),
        ..bridge_core::ClaudeConfig::default()
    };
    let runner = ClaudeRunner::new(config, tx);

    let outcome = runner
        .run_turn(invocation(dir.path(), "will not spawn"), turn_ctx(false))
        .await
        .unwrap();
    assert!(
        matches!(outcome.exit, ExitClass::SpawnFailed(_)),
        "got {:?}",
        outcome.exit
    );
    assert!(outcome.result.is_none());
}

#[tokio::test]
async fn semaphore_accessor_exposes_general_slots() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_fake_claude(dir.path(), &FakeOpts::behavior("success"));
    let (runner, _rx) = runner(&script, 3, 1, 30);
    assert_eq!(runner.semaphore().available_permits(), 2);
}
