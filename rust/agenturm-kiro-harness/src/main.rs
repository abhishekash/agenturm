//! Opt-in Kiro reader using the shared bounded JSONL process runtime.

#![forbid(unsafe_code)]

mod kiro;

use agenturm_jsonl_source::{Result, SourceFormat};
use serde_json::Value;
use std::path::Path;

struct KiroFormat;

impl SourceFormat for KiroFormat {
    type Context = kiro::Context;
    const PLUGIN_ID: &'static str = "example.rust-kiro-harness";
    const GAP_EVENT_PREFIX: &'static str = "kiro-gap";

    fn header_identity(path: &Path, raw: &[u8]) -> Result<Self::Context> {
        kiro::initial_context(path, raw)
    }

    fn checkpoint(context: &Self::Context) -> String {
        kiro::checkpoint(context)
    }

    fn restore(context: &mut Self::Context, checkpoint: &str) -> Result<()> {
        kiro::restore(context, checkpoint)
    }

    fn translate_line(
        raw: &[u8],
        context: &Self::Context,
        origin: &str,
        offset: u64,
    ) -> Result<(Vec<Value>, Self::Context)> {
        kiro::translate_line(raw, context, origin, offset)
    }
}

fn main() {
    // Source paths and content never appear in stderr or process facts.
    if agenturm_jsonl_source::run::<KiroFormat>().is_err() {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn configured_path_is_required_for_identity() {
        let line = br#"{"id":"r1","timestamp":"2026-11-18T12:00:00Z","payload":{"type":"user","content":"private"}}"#;
        assert!(KiroFormat::header_identity(Path::new("/x/other/messages.jsonl"), line).is_err());
        assert!(KiroFormat::header_identity(Path::new("/x/sess_one/other.jsonl"), line).is_err());
        let context =
            KiroFormat::header_identity(Path::new("/x/sess_one/messages.jsonl"), line).unwrap();
        assert_eq!(context.session_id, "sess_one");
    }

    #[test]
    fn same_inode_under_different_session_path_refuses_old_cursor() {
        let root = std::env::temp_dir().join(format!(
            "agenturm-kiro-path-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let first_dir = root.join("sess_first");
        let second_dir = root.join("sess_second");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        let first = first_dir.join("messages.jsonl");
        let second = second_dir.join("messages.jsonl");
        fs::write(&first, b"{\"id\":\"u\",\"timestamp\":\"2026-11-18T12:00:00Z\",\"payload\":{\"type\":\"user\"}}\n").unwrap();
        fs::hard_link(&first, &second).unwrap();
        let initial = agenturm_jsonl_source::poll::<KiroFormat>(
            &first,
            "source",
            "",
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        let cursor = initial["cursor"].as_str().unwrap();
        assert!(
            agenturm_jsonl_source::poll::<KiroFormat>(
                &second,
                "source",
                cursor,
                256,
                agenturm_jsonl_source::max_frame_bytes()
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
