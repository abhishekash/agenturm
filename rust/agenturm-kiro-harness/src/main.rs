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
    use std::io::Write;
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

    #[test]
    fn legacy_pending_call_cursor_promotes_to_k2_on_next_poll() {
        let root = std::env::temp_dir().join(format!(
            "agenturm-kiro-k1-migrate-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let session = root.join("sess_11111111-2222-3333-4444-555555555555");
        fs::create_dir_all(&session).unwrap();
        let path = session.join("messages.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"id\":\"user-one\",\"timestamp\":\"2026-11-18T12:00:00Z\",\"payload\":{\"type\":\"user\"}}\n",
                "{\"id\":\"call-row\",\"timestamp\":\"2026-11-18T12:00:01Z\",\"payload\":{\"type\":\"tool_call\",\"toolCallId\":\"opaque-call\",\"toolName\":\"run_command\",\"args\":{\"command\":\"private command\"}}}\n"
            ),
        )
        .unwrap();
        let first = agenturm_jsonl_source::poll::<KiroFormat>(
            &path,
            "source",
            "",
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        let current = first["cursor"].as_str().unwrap();
        let outer: Vec<_> = current
            .strip_prefix("v2:")
            .unwrap()
            .splitn(7, ':')
            .collect();
        let fields: Vec<_> = outer[6].split(':').collect();
        assert_eq!(fields[0], "k2");
        let current_call = fields[8];
        assert_eq!(current_call.len(), 66);
        let legacy_call = format!("{}{}", &current_call[..24], &current_call[64..]);
        let legacy_checkpoint = format!(
            "k{}:{}:{}:{}:{}:{}:{}:{}",
            fields[1],
            fields[2],
            fields[3],
            fields[4],
            fields[5],
            fields[6],
            fields[7],
            legacy_call
        );
        let legacy_cursor = format!("v2:{}:{}", outer[..6].join(":"), legacy_checkpoint);
        assert!(legacy_cursor.len() <= 800);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"id\":\"result-row\",\"timestamp\":\"2026-11-18T12:00:02Z\",\"payload\":{\"type\":\"tool_result\",\"toolCallId\":\"opaque-call\",\"success\":true}}\n")
            .unwrap();
        let migrated = agenturm_jsonl_source::poll::<KiroFormat>(
            &path,
            "source",
            &legacy_cursor,
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(migrated["records"][0]["payload"]["result"], "succeeded");
        let migrated_outer: Vec<_> = migrated["cursor"]
            .as_str()
            .unwrap()
            .strip_prefix("v2:")
            .unwrap()
            .splitn(7, ':')
            .collect();
        assert!(migrated_outer[6].starts_with("k2:"));
        let encoded = serde_json::to_string(&migrated).unwrap();
        assert!(!encoded.contains("private command"));
        assert!(!migrated_outer[6].contains("opaque-call"));
        fs::remove_dir_all(root).unwrap();
    }
}
