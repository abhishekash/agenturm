//! Opt-in Pi reader using the shared bounded JSONL process runtime.

#![forbid(unsafe_code)]

mod content;
mod directory;
mod pi;

use agenturm_jsonl_source::{Result, SourceFormat};
use serde_json::Value;

struct PiFormat<const CONTENT: bool = false, const COMMAND: bool = false>;

impl<const CONTENT: bool, const COMMAND: bool> SourceFormat for PiFormat<CONTENT, COMMAND> {
    type Context = pi::Context;
    const PLUGIN_ID: &'static str = "example.rust-pi-harness";
    const GAP_EVENT_PREFIX: &'static str = "pi-gap";

    fn header_identity(path: &std::path::Path, raw: &[u8]) -> Result<Self::Context> {
        let legacy = std::env::var_os("AGENTURM_PLUGIN_LEGACY_ADMISSION").as_deref()
            == Some(std::ffi::OsStr::new("1"));
        pi::header_context((!legacy).then_some(path), raw)
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
    let directory_mode = std::env::var_os("AGENTURM_PLUGIN_DISCOVERY_MODE").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let content_mode = std::env::var_os("AGENTURM_PLUGIN_CONTENT_REFS").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let command_mode = std::env::var_os("AGENTURM_PLUGIN_BATCH_FEATURE").as_deref()
        == Some(std::ffi::OsStr::new("pi-command-ref/1"));
    let result =
        if (content_mode && command_mode) || ((content_mode || command_mode) && !directory_mode) {
            std::process::exit(2)
        } else if content_mode {
            agenturm_source_process::run::<directory::ContentDirectoryPoller>()
        } else if command_mode {
            agenturm_source_process::run::<directory::CommandDirectoryPoller>()
        } else if directory_mode {
            agenturm_source_process::run::<directory::DirectoryPoller>()
        } else {
            agenturm_jsonl_source::run::<PiFormat>()
        };
    if result.is_err() {
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
    use sha2::{Digest, Sha256};
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

    #[test]
    fn native_fork_requires_a_verified_sibling_and_a_copied_entry() {
        use std::os::unix::fs::symlink;

        const PARENT: &str = "11111111-1111-4111-8111-111111111111";
        const CHILD: &str = "22222222-2222-4222-8222-222222222222";
        const STAMP: &str = "2026-11-22T12:00:00.000Z";
        let scratch = Scratch::new();
        let parent = scratch
            .0
            .join(format!("2026-11-22T12-00-00-000Z_{PARENT}.jsonl"));
        let child = scratch
            .0
            .join(format!("2026-11-22T12-00-01-000Z_{CHILD}.jsonl"));
        let parent_header = json!({"type":"session","id":PARENT,"timestamp":STAMP});
        let mut child_header =
            json!({"type":"session","id":CHILD,"timestamp":STAMP,"parentSession":parent});
        let copied = json!({"type":"message","id":"first","timestamp":STAMP,
            "message":{"role":"user","content":"private inherited prompt"}});
        fs::write(&parent, format!("{parent_header}\n{copied}\n")).unwrap();
        fs::write(&child, format!("{child_header}\n{copied}\n")).unwrap();

        let first = poll(&child, "harness.pi", "", 64, max_frame_bytes()).unwrap();
        let edge = &first["records"][0];
        assert_eq!(edge["kind"], "relationship");
        assert_eq!(edge["identity"]["session_id"], CHILD);
        assert_eq!(
            edge["payload"],
            json!({"relationship_kind":"forked","parent_session_id":PARENT})
        );
        assert!(!first.to_string().contains("private inherited prompt"));
        assert!(!first.to_string().contains("parentSession"));
        let second = poll(
            &child,
            "harness.pi",
            first["cursor"].as_str().unwrap(),
            64,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(second["records"], json!([]));

        let no_edge = || {
            let batch = poll(&child, "harness.pi", "", 64, max_frame_bytes()).unwrap();
            assert!(
                !batch["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["kind"] == "relationship")
            );
        };
        // A continuation shares the native pointer but has no copied row.
        fs::write(&child, format!("{child_header}\n")).unwrap();
        no_edge();
        fs::write(&child, format!("{child_header}\n{copied}\n")).unwrap();
        let wrong = json!({"type":"session","id":"unrelated","timestamp":STAMP});
        fs::write(&parent, format!("{wrong}\n{copied}\n")).unwrap();
        no_edge();
        let elsewhere = scratch.0.join("outside");
        fs::create_dir(&elsewhere).unwrap();
        let elsewhere = elsewhere.join(parent.file_name().unwrap());
        fs::write(&elsewhere, format!("{parent_header}\n{copied}\n")).unwrap();
        fs::remove_file(&parent).unwrap();
        no_edge();
        symlink(&elsewhere, &parent).unwrap();
        no_edge();
        child_header["parentSession"] = json!(elsewhere);
        fs::write(&child, format!("{child_header}\n{copied}\n")).unwrap();
        no_edge();
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
    fn distant_same_length_body_rewrite_is_a_gap() {
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
                    r#"{{"type":"model_change","id":"model-{index:02}","timestamp":"2026-10-24T12:00:01Z"}}"#
                ),
            );
        }
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        let cursor = first["cursor"].as_str().unwrap();
        assert!(cursor.starts_with("v2:"));
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.len() > 512);
        fs::write(&path, raw.replacen("model-03", "model-XX", 1)).unwrap();
        let after = poll(&path, "source", cursor, 256, max_frame_bytes()).unwrap();
        assert_eq!(facts(&after)[0]["payload"]["code"], "possible_source_gap");
        assert_ne!(facts(&first)[0]["event_id"], facts(&after)[1]["event_id"]);
        assert_ne!(cursor, after["cursor"].as_str().unwrap());
    }

    #[test]
    fn genuine_tail_digest_v1_cursor_requires_a_gap_before_replay() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        for index in 0..20 {
            append(
                &path,
                &format!(
                    r#"{{"type":"model_change","id":"model-{index:02}","timestamp":"2026-10-24T12:00:01Z"}}"#
                ),
            );
        }
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        let encoded = first["cursor"]
            .as_str()
            .unwrap()
            .strip_prefix("v2:")
            .unwrap();
        let fields: Vec<_> = encoded.splitn(7, ':').collect();
        let bytes = fs::read(&path).unwrap();
        let offset = fields[2].parse::<usize>().unwrap();
        assert!(offset > 256);
        let tail = format!("{:x}", Sha256::digest(&bytes[offset - 256..offset]));
        assert_ne!(tail, fields[3]); // The legacy digest is not the v2 prefix digest.
        let old_cursor = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            fields[0], fields[1], fields[2], tail, fields[4], fields[5], fields[6]
        );
        let after = poll(&path, "source", &old_cursor, 256, max_frame_bytes()).unwrap();
        assert_eq!(facts(&after)[0]["payload"]["code"], "possible_source_gap");
        assert!(after["cursor"].as_str().unwrap().starts_with("v2:"));
        let idle = poll(
            &path,
            "source",
            after["cursor"].as_str().unwrap(),
            256,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(facts(&idle).len(), 0); // Migration emits one gap, not one per poll.
    }

    #[test]
    fn cursor_above_verification_ceiling_fails_before_batch() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        let first = poll(&path, "source", "", 256, max_frame_bytes()).unwrap();
        let encoded = first["cursor"]
            .as_str()
            .unwrap()
            .strip_prefix("v2:")
            .unwrap();
        let fields: Vec<_> = encoded.splitn(7, ':').collect();
        let too_large = 512_u64 * 1024 * 1024 + 1;
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(too_large)
            .unwrap(); // Sparse growth; no 512 MiB allocation or read.
        let cursor = format!(
            "v2:{}:{}:{}:{}:{}:{}:{}",
            fields[0], fields[1], too_large, fields[3], fields[4], fields[5], fields[6]
        );
        let error = poll(&path, "source", &cursor, 256, max_frame_bytes()).unwrap_err();
        assert!(error.to_string().contains("verification limit"));
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
