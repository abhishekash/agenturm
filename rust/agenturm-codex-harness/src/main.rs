//! Opt-in Codex rollout reader using the shared bounded JSONL process runtime.

#![forbid(unsafe_code)]

mod codex;
mod directory;

use agenturm_jsonl_source::{Result, SourceFormat};
use serde_json::Value;

struct CodexFormat;

impl SourceFormat for CodexFormat {
    type Context = codex::Context;
    const PLUGIN_ID: &'static str = "example.rust-codex-harness";
    const GAP_EVENT_PREFIX: &'static str = "codex-gap";

    fn header_identity(_path: &std::path::Path, raw: &[u8]) -> Result<Self::Context> {
        codex::header_identity(raw)
    }

    fn checkpoint(context: &Self::Context) -> String {
        context.turn_id.clone()
    }

    fn restore(context: &mut Self::Context, checkpoint: &str) -> Result<()> {
        context.turn_id = checkpoint.to_owned();
        Ok(())
    }

    fn translate_line(
        raw: &[u8],
        context: &Self::Context,
        origin: &str,
        offset: u64,
    ) -> Result<(Vec<Value>, Self::Context)> {
        codex::translate_line(raw, context, origin, offset)
    }
}

fn main() {
    // The host receives a coarse failure only, never source contents or paths.
    let directory_mode = std::env::var_os("AGENTURM_PLUGIN_SOURCE_ROOT")
        .is_some_and(|root| std::path::Path::new(&root).is_dir());
    let result = if directory_mode {
        agenturm_source_process::run::<directory::DirectoryPoller>()
    } else {
        agenturm_jsonl_source::run::<CodexFormat>()
    };
    if result.is_err() {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_jsonl_source::{max_frame_bytes, poll};
    use std::fs;

    #[test]
    fn complete_line_budget_keeps_turn_and_replays_without_duplication() {
        let root = std::env::temp_dir().join(format!("agenturm-codex-unit-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("session.jsonl");
        fs::write(&source, concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-14T10:00:01Z\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-1\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-14T10:00:02Z\",\"payload\":{\"type\":\"item_completed\",\"turn_id\":\"turn-1\",\"item\":{\"type\":\"UserMessage\",\"id\":\"u1\",\"content\":\"private prompt\"}}}\n"
        )).unwrap();
        let first = poll::<CodexFormat>(&source, "source", "", 1, max_frame_bytes()).unwrap();
        assert_eq!(first["records"].as_array().unwrap().len(), 1);
        assert!(first["cursor"].as_str().unwrap().ends_with(":turn-1"));
        // The user row carries two facts and cannot be split by a one-record poll.
        assert!(
            poll::<CodexFormat>(
                &source,
                "source",
                first["cursor"].as_str().unwrap(),
                1,
                max_frame_bytes()
            )
            .is_err()
        );
        let second = poll::<CodexFormat>(
            &source,
            "source",
            first["cursor"].as_str().unwrap(),
            2,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(second["records"].as_array().unwrap().len(), 2);
        assert_eq!(second["records"][0]["identity"]["turn_id"], "turn-1");
        assert!(second["cursor"].as_str().unwrap().ends_with(":turn-1"));
        let idle = poll::<CodexFormat>(
            &source,
            "source",
            second["cursor"].as_str().unwrap(),
            256,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(idle["cursor"], second["cursor"]);
        assert!(idle["records"].as_array().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
