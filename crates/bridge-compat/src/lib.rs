//! Isolation layer for version-fragile Claude Code CLI surfaces.
//!
//! EVERYTHING that touches a CLI surface that can drift between versions
//! lives here and only here: flag spelling, stream-json event shapes, hook
//! payload fields, hook response schemas, settings.json rendering, the env
//! scrub list. A CLI upgrade should touch this crate and its fixtures, and
//! nothing else.
//!
//! Parsing rules (non-negotiable):
//! - unknown JSON fields are ignored
//! - unknown event types parse to `StreamEvent::Unknown`, never an error
//! - malformed lines return `Err(StreamParseError)`, never panic
//!
//! Ground truth: `tests/fixtures/` holds NDJSON and hook payloads captured
//! from claude CLI 2.1.201 on 2026-07-06. Synthetic fixtures (doc-derived,
//! not captured) are marked with a `.synthetic.` infix in the filename.

pub mod hooks;
pub mod invocation;
pub mod settings;
pub mod stream;
pub mod version;

pub use hooks::{HookPayload, hook_response_allow, hook_response_deny, hook_response_passthrough};
pub use invocation::{ClaudeInvocation, OutputFormat, scrubbed_env};
pub use settings::render_worktree_settings;
pub use stream::{
    ApiErrorCategory, ApiRetry, AssistantContent, AssistantEvent, RateLimitEvent, ResultEvent,
    StreamEvent, StreamParseError, SystemInit, UserEvent, parse_line,
};
pub use version::{CliVersion, VersionParseError, VersionStatus, check_version};
