//! Opt-in Pi reader using the shared bounded JSONL process runtime.

#![forbid(unsafe_code)]

mod pi;

use agenturm_jsonl_source::{Result, SourceFormat};
use serde_json::Value;

struct PiFormat;

impl SourceFormat for PiFormat {
    type Context = pi::Context;
    const PLUGIN_ID: &'static str = "example.rust-pi-harness";
    const GAP_EVENT_PREFIX: &'static str = "pi-gap";

    fn header_identity(raw: &[u8]) -> Result<Self::Context> {
        pi::header_identity(raw)
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
        pi::translate_line(raw, context, origin, offset)
    }
}

fn main() {
    // The host receives a coarse failure only, never source contents or paths.
    if agenturm_jsonl_source::run::<PiFormat>().is_err() {
        std::process::exit(2);
    }
}

#[cfg(test)]
fn poll(
    path: &std::path::Path,
    source_id: &str,
    cursor: &str,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    agenturm_jsonl_source::poll::<PiFormat>(path, source_id, cursor, max_records, max_bytes)
}

#[cfg(test)]
use agenturm_jsonl_source::max_frame_bytes;
#[cfg(test)]
use agenturm_jsonl_source::{MAX_CURSOR, MAX_STRUCTURAL_TOKENS, batch, structural_tokens};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agenturm-pi-reader-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn source(&self) -> PathBuf {
            self.0.join("session.jsonl")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn append(path: &Path, line: &str) {
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap(),
            "{line}"
        )
        .unwrap();
    }

    fn facts(value: &Value) -> &[Value] {
        value["records"].as_array().unwrap()
    }

    #[test]
    fn line_facts_are_atomic_and_turn_survives_ack_restart() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"pi-session"}"#);
        append(
            &path,
            r#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user","content":[{"type":"text","text":"private request"}]}}"#,
        );
        append(
            &path,
            r#"{"type":"message","id":"assistant-1","timestamp":"2026-10-24T12:00:01Z","message":{"role":"assistant","stopReason":"toolUse","usage":{"input":8,"output":3},"content":[{"type":"toolCall","id":"call-1","name":"read"},{"type":"toolCall","id":"call-2","name":"bash"}]}}"#,
        );
        let first = poll(&path, "source", "", 3, max_frame_bytes()).unwrap();
        assert_eq!(facts(&first).len(), 2); // Header plus entire user line.
        let cursor = first["cursor"].as_str().unwrap();
        assert!(cursor.len() <= MAX_CURSOR);
        assert!(cursor.ends_with(":user-1"));
        let second = poll(&path, "source", cursor, 5, max_frame_bytes()).unwrap();
        assert_eq!(facts(&second).len(), 5); // Assistant line is not split.
        assert!(
            facts(&second)
                .iter()
                .all(|fact| fact["identity"]["turn_id"] == "user-1")
        );
        assert_eq!(facts(&second)[2]["identity"]["operation_id"], "call-1");
        let second_cursor = second["cursor"].as_str().unwrap();
        assert_eq!(
            facts(&poll(&path, "source", second_cursor, 256, max_frame_bytes()).unwrap()).len(),
            0
        );
        assert!(poll(&path, "source", cursor, 3, max_frame_bytes()).is_err());
        assert!(poll(&path, "source", cursor, 256, 300).is_err());
    }

    #[test]
    fn replacement_emits_gap_and_resets_turn() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        append(
            &path,
            r#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#,
        );
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        let replacement = scratch.0.join("next.jsonl");
        append(&replacement, r#"{"type":"session","id":"second"}"#);
        append(
            &replacement,
            r#"{"type":"message","id":"user-2","timestamp":"2026-10-24T12:01:00Z","message":{"role":"user"}}"#,
        );
        fs::rename(replacement, &path).unwrap();
        let after = poll(
            &path,
            "source",
            first["cursor"].as_str().unwrap(),
            256,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(facts(&after)[0]["payload"]["code"], "possible_source_gap");
        assert_eq!(facts(&after)[1]["identity"]["session_id"], "second");
        assert_eq!(facts(&after)[1]["identity"]["turn_id"], "user-2");
        assert_ne!(facts(&after)[1]["event_id"], facts(&first)[0]["event_id"]);
    }

    #[test]
    fn distant_header_rewrite_is_a_gap_and_cannot_keep_old_turn() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        append(
            &path,
            r#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#,
        );
        for index in 0..20 {
            append(
                &path,
                &format!(
                    r#"{{"type":"model_change","id":"model-{index}","timestamp":"2026-10-24T12:00:01Z"}}"#
                ),
            );
        }
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        let before = first["cursor"].as_str().unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        fs::write(&path, raw.replacen("first", "other", 1)).unwrap();
        let after = poll(&path, "source", before, 256, max_frame_bytes()).unwrap();
        assert_eq!(facts(&after)[0]["payload"]["code"], "possible_source_gap");
        assert_eq!(facts(&after)[1]["identity"]["session_id"], "other");
        assert_ne!(facts(&first)[0]["event_id"], facts(&after)[1]["event_id"]);
    }

    #[test]
    fn partial_and_oversized_lines_do_not_advance() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        fs::write(
            &path,
            format!(
                "{}\n{{\"type\":\"message\"",
                r#"{"type":"session","id":"first"}"#
            ),
        )
        .unwrap();
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        let cursor = first["cursor"].as_str().unwrap();
        assert!(facts(&first).is_empty());
        assert!(cursor.len() > 64);
        let idle = poll(&path, "source", cursor, 256, max_frame_bytes()).unwrap();
        assert_eq!(idle["cursor"], cursor);
        assert!(facts(&idle).is_empty());
        append(
            &path,
            r#", "id":"u","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#,
        );
        let complete = poll(&path, "source", cursor, 256, max_frame_bytes()).unwrap();
        assert_eq!(facts(&complete).len(), 2);
        fs::write(&path, b"{\"type\":\"session\"").unwrap();
        assert!(
            poll(
                &path,
                "source",
                complete["cursor"].as_str().unwrap(),
                256,
                max_frame_bytes()
            )
            .is_err()
        );
    }

    #[test]
    fn structural_budget_pages_whole_pi_lines() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"pi-session"}"#);
        let blocks: Vec<Value> = (0..126)
            .map(|index| {
                json!({"type":"toolCall",
            "id":format!("call-{index}"),"name":"read"})
            })
            .collect();
        let row = json!({"type":"message","id":"assistant-1",
            "timestamp":"2026-10-24T12:00:01Z",
            "message":{"role":"assistant","stopReason":"toolUse","content":blocks}});
        append(&path, &row.to_string());
        let mut second = row;
        second["id"] = json!("assistant-2");
        for block in second["message"]["content"].as_array_mut().unwrap() {
            block["id"] = json!(format!("next-{}", block["id"].as_str().unwrap()));
        }
        append(&path, &second.to_string());
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        assert_eq!(facts(&first).len(), 128);
        let resumed = poll(
            &path,
            "source",
            first["cursor"].as_str().unwrap(),
            256,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(facts(&resumed).len(), 128);
        let combined: Vec<Value> = facts(&first)
            .iter()
            .chain(facts(&resumed).iter())
            .cloned()
            .collect();
        assert_eq!(combined.len(), 256);
        let encoded = serde_json::to_vec(&batch("source", "cursor", &combined)).unwrap();
        assert!(encoded.len() < max_frame_bytes());
        assert!(structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS);
    }
}
