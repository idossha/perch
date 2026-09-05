pub mod claude;
pub mod codex;
pub mod pi;

use crate::model::{Harness, ParsedEvent};

/// The pi extension perch installs, embedded in the binary.
pub const PI_EXTENSION: &str = include_str!("perch.pi.ts");

/// Parse one raw hook payload for a harness into a neutral event.
///
/// `Ok(None)` means "understood, but nothing to record" (a subagent event, or
/// an event type perch does not track).
pub fn parse(harness: Harness, raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    match harness {
        Harness::Claude => claude::parse(raw),
        Harness::Codex => codex::parse(raw),
        Harness::Pi => pi::parse(raw),
    }
}
