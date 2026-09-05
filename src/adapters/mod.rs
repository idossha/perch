pub mod claude;
pub mod stub;

use crate::model::{Harness, ParsedEvent};

/// Parse one raw hook payload for a harness into a neutral event.
///
/// `Ok(None)` means "understood, but nothing to record" (a subagent event, or
/// an event type perch does not track).
pub fn parse(harness: Harness, raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    match harness {
        Harness::Claude => claude::parse(raw),
        Harness::Codex => stub::parse(Harness::Codex, raw),
        Harness::Pi => stub::parse(Harness::Pi, raw),
    }
}
