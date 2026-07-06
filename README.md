# Bridge

A Star Trek bridge themed agentic harness in Rust.

A **Captain** agent decomposes a mission into concurrent workstreams; specialist **station** agents execute them in isolated git worktrees, each turn driven by the locally installed Claude Code CLI in headless mode (your existing subscription login, never the raw API). Every tool call is adjudicated mid-run by a fail-closed hook veto system, every completed workstream must survive the **Kobayashi Maru** adversarial tester before it may merge, and a native GUI lets you observe and command all of it in real time.

```
Mission objective
      |
   Captain  -- plans a graph of workstreams (structured output)
      |
      v  (topological order, concurrent)
   Helm x N  -- each in its own worktree on bridge/<mission>/<workstream>
      |          every Bash/Edit/Write screened by Tactical hooks (fail closed)
      v
   Kobayashi Maru  -- fresh throwaway worktree, tries to break the change
      |               breached -> Helm fix -> re-attack (max N rounds)
      v
   Merge queue  -- strictly serialized: rebase -> checks -> YOUR confirmation -> local merge
      |
   Comms  -- final mission report
```

## Why it is built this way

- **No direct API calls.** Each agent turn spawns `claude -p --output-format stream-json --verbose` inside the workstream's worktree, using your local subscription auth. `ANTHROPIC_API_KEY` and inherited `CLAUDE_*` session vars are scrubbed from every child so subscription OAuth is used; `--bare` is never passed (it would skip OAuth). Sessions are captured per workstream and continued with `--resume` from the same directory.
- **Fail-closed guardrails.** For each worktree, Engineering installs a `.claude/settings.json` whose PreToolUse/PostToolUse/Stop hooks run a bundled helper binary. The helper POSTs each hook to a local control server and prints its decision; if the server is unreachable, times out, or the payload is unreadable, the helper **denies** the tool call itself and still exits 0. (Claude Code's native HTTP hooks fail *open* on connection error, which is why the helper exists.)
- **Version resilience.** Everything version-fragile - CLI flag spelling, stream-json event shapes, hook payload/response schemas, settings rendering - lives only in `bridge-compat`, verified against fixtures captured from a known CLI version. Parsers are schema-tolerant: unknown fields ignored, unknown event types skipped, malformed input errors instead of panicking. Preflight records `claude --version` and the GUI shows a compatibility banner outside the tested range.
- **Your data stays put.** Worktrees live under `~/.bridge/<repo>/worktrees`, never inside the target checkout. Merges to `main` are local and require explicit GUI confirmation; pushing to a remote is never automatic. The Kobayashi tester can only write under `tests/adversarial/` (enforced by the hook path policy), so it can never modify the code it attacks.

## Workspace

| Crate | Role |
| --- | --- |
| `bridge-core` | Shared types: ids, config, events, plan graph, orders, battle reports, wire protocol. No async. |
| `bridge-compat` | The only crate that touches version-fragile CLI surfaces (flags, stream-json, hook schemas, settings) |
| `bridge-engine` | Claude process runner: two-tier semaphore, timeouts, session registry, budgets, rate-limit detection, preflight |
| `bridge-git` | Worktree lifecycle, rebase and local merge primitives (shells out to real `git`) |
| `bridge-tactical` | Policy engine (prime directives, config rules, Red Alert), escalation broker, axum control server, order screening |
| `bridge-hook-helper` | Fail-closed hook forwarder binary installed into every worktree |
| `bridge-computer` | Ship's Computer: SQLite persistence (missions, turns, hook decisions, battle reports, sessions, usage, findings memory) |
| `bridge-stations` | Station profiles and prompts, Captain orchestration, mission state machine, merge queue, Kobayashi Maru |
| `bridge-app` | The `bridge` GUI binary (eframe/egui) plus the composition root |

The stations orchestration is decoupled from the live crates by four port traits (`TurnPort`/`GitPort`/`TacticalPort`/`ComputerPort`), so the entire mission state machine is unit-tested with mocks and zero subprocesses.

## Requirements

- Rust (2024 edition; built with 1.96).
- The Claude Code CLI on `PATH`, logged in with an active subscription. Tested against **2.1.201**; the tested range is declared in config and enforced at preflight.
- `git`.

## Quickstart

```sh
cargo build --release

# Point Bridge at the repository a mission should work on.
cp bridge.example.toml /path/to/target-repo/bridge.toml   # optional; defaults are built in
./target/release/bridge --repo /path/to/target-repo
```

Type a mission objective in the GUI. The Captain plans it, workstreams run concurrently, escalations and merge confirmations appear as prompts you answer in the GUI. **Red Alert** (a header toggle) escalates every tool call to you.

### Headless mode

For CI or scripted verification, drive one mission without a display. It prints every event as a JSON line and auto-approves escalations and merges:

```sh
./target/release/bridge --repo /path/to/target-repo --headless-smoke \
  "Add a CONTRIBUTING.md with a short contribution guide"
```

## Configuration

All keys are optional; see [bridge.example.toml](bridge.example.toml) for the annotated defaults. Highlights:

- `[claude]` - `model`, `kobayashi_model`, `max_concurrent` (semaphore, default 3), `kobayashi_reserved_slots`, `turn_timeout_secs`, tested version range.
- `[budgets]` - `max_total_turns`, `max_turns_per_workstream`, `max_kobayashi_rounds`, optional `max_wall_clock_secs`. Exhaustion pauses the mission; you extend or wind down from the GUI.
- `[tactical]` - destructive command patterns, protected path patterns, network egress policy, optional deep scan, Red Alert default, escalation timeout. Prime directives are hardcoded and cannot be disabled here.
- `[merge]` - `rebase` (default) or `merge-commit`.
- `[worktrees]` - override the root, keep-on-failure.
- `[linear]` - optional MCP sync; reads auto-allowed, writes escalate unless they match expected sync events, deletes always escalate.

## Compatibility policy

Bridge depends on unstable CLI surfaces. When you upgrade the Claude Code CLI:

1. Bump `tested_version_min`/`tested_version_max` in your config.
2. Recapture the fixtures in `crates/bridge-compat/tests/fixtures/` (the provenance commands are in that directory's `README.md`) and run `cargo test -p bridge-compat`.
3. If a parser test fails, the fix belongs in `bridge-compat` and nowhere else.

## Development

```sh
cargo test --workspace     # unit + integration (no real CLI calls; engine tests use a fake claude)
cargo clippy --workspace --all-targets
```

Only the headless smoke run and manual GUI use invoke the real `claude` binary; the automated suite is hermetic.
