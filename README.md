# Bridge

A Star Trek bridge themed agentic harness in Rust.
A Captain agent decomposes a mission into concurrent workstreams; station agents execute them in isolated git worktrees, driven by the locally installed Claude Code CLI in headless mode (your existing subscription login, never the raw API).
Every tool call is adjudicated mid-run by a fail-closed hook veto system, every completed workstream must survive the Kobayashi Maru adversarial tester before it may merge, and a native GUI lets you observe and command all of it in real time.

Status: under construction. See [docs/superpowers/plans/2026-07-06-bridge.md](docs/superpowers/plans/2026-07-06-bridge.md) for the implementation plan and architecture, and `bridge.example.toml` for configuration.

## Workspace

| Crate | Role |
| --- | --- |
| `bridge-core` | Shared types: ids, config, events, plan graph, orders, battle reports, wire protocol |
| `bridge-compat` | The only crate that touches version-fragile CLI surfaces (flags, stream-json, hook schemas) |
| `bridge-engine` | Claude process runner: semaphore, timeouts, sessions, budgets, rate-limit detection |
| `bridge-git` | Worktree lifecycle, rebase and local merge primitives |
| `bridge-tactical` | Policy engine, escalation broker, hook control server |
| `bridge-hook-helper` | Fail-closed hook forwarder binary installed into every worktree |
| `bridge-computer` | Ship's Computer: SQLite persistence |
| `bridge-stations` | Station profiles, Captain orchestration, merge queue, Kobayashi Maru |
| `bridge-app` | The `bridge` GUI binary (eframe/egui) |
