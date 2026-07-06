//! Pre-dispatch screening: Tactical reviews order text and tool lists
//! BEFORE any claude process spawns.
//!
//! This is best-effort injection screening; the real security boundary is
//! the capability limits (hooks + static tool policy), not detection.

use bridge_core::Order;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenResult {
    Cleared,
    /// Order rejected outright (tool list exceeds the station profile).
    Rejected { reason: String },
    /// Suspicious content found in the prompt (likely injection from
    /// embedded external content); ship it to the GUI as an escalation
    /// before dispatch.
    NeedsReview { flags: Vec<String> },
}

/// Screen an order against its station's allowed tool surface and scan the
/// prompt for embedded-instruction red flags.
///
/// Checks:
/// - every entry of `order.allowed_tools` must be within
///   `station_allowed_tools` (pattern-for-pattern subset by tool name);
/// - prompt scans: "ignore previous instructions" family, requests to
///   read/exfiltrate credentials, requests to disable hooks/settings,
///   base64 blobs over a size threshold, instructions to push to remotes.
pub fn screen_order(order: &Order, station_allowed_tools: &[String]) -> ScreenResult {
    todo!()
}
