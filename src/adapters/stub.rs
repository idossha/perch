//! Placeholder adapters for codex and pi (phase 3).

use crate::model::{Harness, ParsedEvent};

pub fn parse(harness: Harness, _raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    anyhow::bail!(
        "the {} adapter is not implemented yet (phase 3)",
        harness.as_str()
    )
}
