//! Opt-in Claude Code main-transcript reader on the shared bounded JSONL runtime.

#![forbid(unsafe_code)]

mod claude;

use agenturm_jsonl_source::{Result, SourceFormat};
use serde_json::Value;

struct ClaudeFormat<const LEGACY: bool>;

impl<const LEGACY: bool> SourceFormat for ClaudeFormat<LEGACY> {
    type Context = claude::Context;
    const PLUGIN_ID: &'static str = "example.rust-claude-harness";
    const GAP_EVENT_PREFIX: &'static str = "claude-gap";

    fn header_identity(_path: &std::path::Path, raw: &[u8]) -> Result<Self::Context> {
        claude::header_identity(raw)
    }

    fn checkpoint(context: &Self::Context) -> String {
        claude::checkpoint(context)
    }

    fn restore(context: &mut Self::Context, checkpoint: &str) -> Result<()> {
        claude::restore(context, checkpoint)
    }

    fn translate_line(
        raw: &[u8],
        context: &Self::Context,
        origin: &str,
        offset: u64,
    ) -> Result<(Vec<Value>, Self::Context)> {
        let (mut facts, context) = claude::translate_line(raw, context, origin, offset)?;
        if LEGACY {
            facts.retain(|row| row["kind"] != "mcp_call");
        }
        Ok((facts, context))
    }
}

fn main() {
    let legacy = std::env::var_os("AGENTURM_PLUGIN_LEGACY_ADMISSION").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let outcome = if legacy {
        agenturm_jsonl_source::run::<ClaudeFormat<true>>()
    } else {
        agenturm_jsonl_source::run::<ClaudeFormat<false>>()
    };
    if outcome.is_err() {
        // Host receives only a coarse process failure; no private row or path.
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_revision_is_bounded_and_gap_identity_is_separate() {
        assert!(claude::revision(0).is_ok());
        assert!(claude::revision((1 << 31) - 1).is_ok());
        assert!(claude::revision(1 << 31).is_err());
    }
}
