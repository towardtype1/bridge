//! bridge-hook-helper: the fail-closed hook forwarder.
//!
//! Claude Code runs this binary for PreToolUse/PostToolUse/Stop hooks in
//! Bridge-managed worktrees. It:
//! 1. reads the hook JSON payload from stdin (hard cap 10 MiB),
//! 2. reads `BRIDGE_SERVER_URL`, `BRIDGE_WORKSTREAM_ID`, `BRIDGE_TOKEN`
//!    from env (argv overrides: `--server-url`, `--workstream`, `--token`),
//! 3. POSTs `{workstream_id, payload}` to `<url>/hook` with
//!    `Authorization: Bearer <token>`, timeout `BRIDGE_HELPER_TIMEOUT_MS`
//!    (default 570_000 ms; must stay under the installed hook timeout),
//! 4. prints the response body verbatim to stdout and exits 0.
//!
//! FAIL CLOSED: on ANY failure (missing env, unreadable stdin, connect
//! error, non-2xx, timeout, empty body) it prints a deny decision itself
//! and still exits 0 so the deny JSON is honored:
//! - PreToolUse payloads: permissionDecision "deny" with the failure
//!   reason (the run continues, the tool call does not).
//! - PostToolUse/Stop payloads (nothing to gate): `{}` so observability
//!   loss never corrupts the run.
//! The payload's own `hook_event_name` decides which shape to print; if
//! even that is unreadable, print the PreToolUse deny (safest).
//!
//! Keep this binary tiny and fast: no tokio, no async, blocking ureq only.

fn main() {
    todo!()
}
