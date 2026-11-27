//! Opt-in Codex rollout reader using the shared bounded JSONL process runtime.

#![forbid(unsafe_code)]

mod codex;
mod directory;

use agenturm_jsonl_source::{Result, SourceFormat};
use serde_json::Value;
use std::ffi::OsStr;

struct CodexFormat<const LEGACY: bool = false>;

impl<const LEGACY: bool> SourceFormat for CodexFormat<LEGACY> {
    type Context = codex::Context;
    const PLUGIN_ID: &'static str = "example.rust-codex-harness";
    const GAP_EVENT_PREFIX: &'static str = "codex-gap";

    fn header_identity(_path: &std::path::Path, raw: &[u8]) -> Result<Self::Context> {
        codex::header_identity(raw)
    }

    fn checkpoint(context: &Self::Context) -> String {
        context.checkpoint()
    }

    fn restore(context: &mut Self::Context, checkpoint: &str) -> Result<()> {
        context.restore(checkpoint);
        Ok(())
    }

    fn resume_verified_header(
        raw: &[u8],
        context: &mut Self::Context,
        origin: &str,
    ) -> Result<Option<Value>> {
        if LEGACY {
            Ok(None)
        } else {
            codex::resume_verified_header(raw, context, origin)
        }
    }

    fn translate_line(
        raw: &[u8],
        context: &Self::Context,
        origin: &str,
        offset: u64,
    ) -> Result<(Vec<Value>, Self::Context)> {
        let (mut facts, mut context) = codex::translate_line(raw, context, origin, offset)?;
        if LEGACY {
            context.omit_discovery();
            facts.retain_mut(|fact| match fact["kind"].as_str() {
                Some(
                    "file_edit"
                    | "mcp_call"
                    | "pull_request_raised"
                    | "internal_review"
                    | "native_request"
                    | "session_discovered",
                ) => false,
                Some("check") => {
                    if let Some(identity) = fact["identity"].as_object_mut() {
                        identity.remove("operation_id");
                    }
                    true
                }
                _ => true,
            });
        }
        Ok((facts, context))
    }
}

fn main() {
    // The host receives a coarse failure only, never source contents or paths.
    let directory_mode =
        std::env::var_os("AGENTURM_PLUGIN_DISCOVERY_MODE").as_deref() == Some(OsStr::new("1"));
    let legacy =
        std::env::var_os("AGENTURM_PLUGIN_LEGACY_ADMISSION").as_deref() == Some(OsStr::new("1"));
    let result = match (directory_mode, legacy) {
        (true, false) => agenturm_source_process::run::<directory::DirectoryPoller<false>>(),
        (true, true) => agenturm_source_process::run::<directory::DirectoryPoller<true>>(),
        (false, false) => agenturm_jsonl_source::run::<CodexFormat>(),
        (false, true) => agenturm_jsonl_source::run::<CodexFormat<true>>(),
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
    fn verified_legacy_header_cursor_gains_one_discovery_and_replays_stably() {
        let root =
            std::env::temp_dir().join(format!("agenturm-codex-header-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let cleanup = Cleanup(root);
        let file = cleanup.0.join("thread.jsonl");
        let header = concat!(
            r#"{"type":"session_meta","payload":{"id":"thread-1","timestamp":"1970-01-01T00:00:01.000000007Z","cwd":"/private/fixture","instructions":"private text"}}"#,
            "\n"
        );
        fs::write(&file, header).unwrap();
        let old = poll::<CodexFormat<true>>(&file, "fixture", "", 1, max_frame_bytes()).unwrap();
        let old_cursor = old["cursor"].as_str().unwrap();
        assert!(old_cursor.ends_with(':'));
        assert_eq!(old["records"], serde_json::json!([]));
        let derive =
            |cursor| poll::<CodexFormat>(&file, "fixture", cursor, 1, max_frame_bytes()).unwrap();
        let upgraded = derive(old_cursor);
        let first = derive("");
        assert_eq!(upgraded, first);
        assert_eq!(derive(old_cursor), upgraded);
        let rows = upgraded["records"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kind"], "session_discovered");
        assert_eq!(rows[0]["time_ns"], "1000000007");
        assert_eq!(
            rows[0]["identity"],
            serde_json::json!({"session_id":"thread-1"})
        );
        assert_eq!(rows[0]["payload"], serde_json::json!({}));
        assert!(!upgraded.to_string().contains("private"));
        let new_cursor = upgraded["cursor"].as_str().unwrap();
        let old_proof = agenturm_jsonl_source::cursor_proof(old_cursor)
            .unwrap()
            .unwrap();
        let new_proof = agenturm_jsonl_source::cursor_proof(new_cursor)
            .unwrap()
            .unwrap();
        assert_eq!(old_proof.offset, header.len() as u64);
        assert_eq!(new_proof.offset, old_proof.offset);
        assert_eq!(new_proof.digest, old_proof.digest);
        assert_eq!(new_proof.generation, old_proof.generation);
        let idle = derive(new_cursor);
        assert_eq!(idle["records"], serde_json::json!([]));
        assert_eq!(idle["cursor"], new_cursor);

        let context_only = concat!(
            r#"{"type":"turn_context","timestamp":"1970-01-01T00:00:02Z","payload":{"model":"fixture"}}"#,
            "\n"
        );
        fs::write(&file, format!("{header}{context_only}")).unwrap();
        let legacy_past_header =
            poll::<CodexFormat<true>>(&file, "fixture", old_cursor, 1, max_frame_bytes()).unwrap();
        assert_eq!(legacy_past_header["records"], serde_json::json!([]));
        let beyond = legacy_past_header["cursor"].as_str().unwrap();
        assert!(
            agenturm_jsonl_source::cursor_proof(beyond)
                .unwrap()
                .unwrap()
                .offset
                > old_proof.offset
        );
        let continued = derive(beyond);
        assert_eq!(continued["records"], serde_json::json!([]));
        assert_eq!(continued["cursor"], beyond);

        fs::rename(&file, cleanup.0.join("previous.jsonl")).unwrap();
        fs::write(&file, header).unwrap();
        let replacement =
            poll::<CodexFormat>(&file, "fixture", old_cursor, 3, max_frame_bytes()).unwrap();
        assert_eq!(
            replacement["records"][0]["payload"]["code"],
            "possible_source_gap"
        );
        assert_eq!(replacement["records"][1]["kind"], "session_discovered");
        assert_ne!(replacement["records"][1]["event_id"], rows[0]["event_id"]);

        let guardian = header.replace(
            "\"id\":\"thread-1\"",
            "\"id\":\"review\",\"thread_source\":\"guardian_review\"",
        );
        fs::write(&file, guardian).unwrap();
        let rewritten = poll::<CodexFormat>(
            &file,
            "fixture",
            replacement["cursor"].as_str().unwrap(),
            3,
            max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(
            rewritten["records"][0]["payload"]["code"],
            "possible_source_gap"
        );
        assert_eq!(rewritten["records"][1]["kind"], "internal_review");
        let old_guardian =
            poll::<CodexFormat<true>>(&file, "fixture", "", 1, max_frame_bytes()).unwrap();
        let unchanged_guardian = derive(old_guardian["cursor"].as_str().unwrap());
        assert_eq!(unchanged_guardian["records"], serde_json::json!([]));
    }

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
