# Fixture provenance

Captured live on 2026-07-06 from claude CLI **2.1.201** on macOS (subscription auth, `apiKeySource: "none"`), paths sanitized.

| File | Source |
| --- | --- |
| `stream_basic.ndjson` | `claude -p "<echo LCARS-OK task>" --output-format stream-json --verbose --max-turns 3 --model haiku --allowedTools "Bash(echo *)" --setting-sources project` |
| `stream_resume.ndjson` | Same session resumed with `--resume <session-id>`; proves session_id is retained across resume |
| `hook_pretooluse.json` | PreToolUse payload dumped by a project-settings command hook during the run above |
| `hook_posttooluse.json` | PostToolUse payload, same run |
| `hook_stop.json` | Stop payload, same run |
| `stream_errors.synthetic.ndjson` | DOC-DERIVED (not captured): one `system/api_retry` (`error: "rate_limit"`, status 429), one `rate_limit_event` with `status: "rejected"`, one `result` with subtype `error_max_turns` / `is_error: true` |

Files with `.synthetic.` in the name are DOC-DERIVED, not captured: shapes taken from https://code.claude.com/docs (hooks + headless pages, fetched 2026-07-06) for cases we cannot cheaply trigger (error result subtypes, `system/api_retry`, rate-limited `rate_limit_event`).
When upgrading the tested CLI range, recapture the live files with the commands above and diff.
