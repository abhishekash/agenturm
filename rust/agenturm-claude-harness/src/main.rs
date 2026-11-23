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
        let mut context = claude::header_identity(raw)?;
        if LEGACY {
            context.use_legacy_admission();
        }
        Ok(context)
    }

    fn checkpoint(context: &Self::Context) -> String {
        claude::checkpoint(context)
    }

    fn restore(context: &mut Self::Context, checkpoint: &str) -> Result<()> {
        claude::restore(context, checkpoint)
    }

    fn resume_verified_header(
        raw: &[u8],
        context: &mut Self::Context,
        origin: &str,
    ) -> Result<Option<Value>> {
        if LEGACY {
            Ok(None)
        } else {
            claude::resume_verified_header(raw, context, origin)
        }
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
    use agenturm_jsonl_source::{cursor_proof, max_frame_bytes, poll};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    const PRELUDE: &str = concat!(
        r#"{"type":"file-history-snapshot","messageId":"earlier-message","isSnapshotUpdate":false,"snapshot":{"messageId":"earlier-message","trackedFileBackups":{}}}"#,
        "\n"
    );

    fn user(session: Option<&str>, uuid: &str) -> String {
        let mut event = json!({"type":"user","uuid":uuid,"timestamp":"2026-10-24T12:00:00Z",
            "message":{"content":"synthetic private content"}});
        if let Some(session) = session {
            event["sessionId"] = json!(session);
        }
        format!("{event}\n")
    }

    #[test]
    fn timestamped_claude_metadata_backfills_only_verified_header_and_preserves_legacy() {
        const HEADER: &str = concat!(
            r#"{"type":"custom-title","sessionId":"claude-native","timestamp":"1970-01-01T00:00:01.000000009Z","customTitle":"private caption","cwd":"/private/root"}"#,
            "\n"
        );
        let path = std::env::temp_dir().join(format!(
            "agenturm-claude-presence-{}.jsonl",
            std::process::id(),
        ));
        fs::write(&path, HEADER).unwrap();
        let read = |cursor| {
            poll::<ClaudeFormat<false>>(
                &path,
                "harness.claude.fixture",
                cursor,
                4,
                max_frame_bytes(),
            )
        };
        let prior =
            poll::<ClaudeFormat<true>>(&path, "harness.claude.fixture", "", 4, max_frame_bytes())
                .unwrap();
        assert_eq!(prior["records"].as_array().unwrap().len(), 1);
        assert_eq!(prior["records"][0]["kind"], "diagnostic");
        let committed_before = prior["cursor"].as_str().unwrap();
        let output = read(committed_before).unwrap();
        assert_eq!(read("").unwrap(), output);
        assert_eq!(read(committed_before).unwrap(), output);
        assert_eq!(output["records"].as_array().unwrap().len(), 1);
        let row = &output["records"][0];
        assert_eq!(row["kind"], "session_discovered");
        assert_eq!(row["revision"], 0);
        assert_eq!(row["time_ns"], "1000000009");
        assert_eq!(row["identity"], json!({"session_id":"claude-native"}));
        assert_eq!(row["payload"], json!({}));
        assert!(!output.to_string().contains("private caption"));
        assert!(!output.to_string().contains("/private/root"));
        let committed = output["cursor"].as_str().unwrap();
        let proof = cursor_proof(committed).unwrap().unwrap();
        assert_eq!(proof.offset, HEADER.len() as u64);
        assert_eq!(
            proof.digest,
            format!("{:x}", Sha256::digest(HEADER.as_bytes()))
        );
        assert_ne!(committed_before, committed);
        assert_eq!(read(committed).unwrap()["records"], json!([]));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn claude_user_stub_is_neutral_but_a_real_request_keeps_its_existing_facts() {
        for (raw, kinds) in [
            (
                br#"{"type":"user","sessionId":"claude-native","timestamp":"1970-01-01T00:00:02Z","message":{"content":[]}}"#.as_slice(),
                &["session_discovered"][..],
            ),
            (
                br#"{"type":"user","sessionId":"claude-native","timestamp":"1970-01-01T00:00:02Z","message":{"content":"private request"}}"#.as_slice(),
                &["message", "lifecycle"][..],
            ),
            (
                br#"{"type":"agent-name","sessionId":"claude-native","agentName":"private"}"#.as_slice(),
                &["diagnostic"][..],
            ),
            (
                br#"{"type":"agent-name","sessionId":"claude-native","timestamp":"1970-01-01T00:00:01Z","timestamp":"1970-01-01T00:00:02Z","agentName":"private"}"#.as_slice(),
                &["diagnostic"][..],
            ),
        ] {
            let context = claude::header_identity(raw).unwrap();
            let (facts, _) = claude::translate_line(raw, &context, "fixture", 0).unwrap();
            assert_eq!(facts.iter().map(|row| row["kind"].as_str().unwrap()).collect::<Vec<_>>(), kinds);
        }

        let prelude = claude::header_identity(PRELUDE.as_bytes()).unwrap();
        let raw = br#"{"type":"user","sessionId":"claude-native","timestamp":"1970-01-01T00:00:02Z","message":{"content":[]}}"#;
        let (facts, next) =
            claude::translate_line(raw, &prelude, "fixture", PRELUDE.len() as u64).unwrap();
        assert_eq!(facts[0]["kind"], "session_discovered");
        assert_eq!(facts[0]["identity"], json!({"session_id":"claude-native"}));
        assert!(claude::checkpoint(&next).starts_with("p13:claude-native:d."));
    }

    #[test]
    fn initial_metadata_prelude_binds_exact_user_identity_and_resumes_without_reopening() {
        let source = std::env::temp_dir().join(format!(
            "agenturm-claude-prelude-{}.jsonl",
            std::process::id()
        ));
        let session = "claude:with-delimiters";
        let next = user(Some(session), "first-user");
        fs::write(&source, format!("{PRELUDE}{}", next.trim_end())).unwrap();
        let read = |cursor| {
            poll::<ClaudeFormat<false>>(
                &source,
                "harness.claude.fixture",
                cursor,
                256,
                max_frame_bytes(),
            )
        };
        let before = read("").unwrap();
        let checkpoint = before["cursor"].as_str().unwrap();
        assert_eq!(before["records"], json!([]));
        let first = cursor_proof(checkpoint).unwrap().unwrap();
        assert_eq!(first.version, 2);
        assert_eq!(first.offset, PRELUDE.len() as u64);
        assert_eq!(
            first.header_digest,
            format!("{:x}", Sha256::digest(PRELUDE.as_bytes()))
        );
        assert_eq!(first.digest, first.header_digest);

        let mut writer = OpenOptions::new().append(true).open(&source).unwrap();
        writer.write_all(b"\n").unwrap();
        let first_user = read(checkpoint).unwrap();
        let new_cursor = first_user["cursor"].as_str().unwrap();
        let second = cursor_proof(new_cursor).unwrap().unwrap();
        assert_eq!(second.header_digest, first.header_digest);
        assert_eq!(second.generation, first.generation);
        assert_eq!(second.offset, (PRELUDE.len() + next.len()) as u64);
        assert_eq!(
            second.digest,
            format!("{:x}", Sha256::digest(format!("{PRELUDE}{next}")))
        );
        for record in first_user["records"].as_array().unwrap() {
            assert_eq!(record["identity"]["session_id"], session);
        }
        assert_eq!(first_user["records"].as_array().unwrap().len(), 2);
        assert!(!first_user.to_string().contains("synthetic private content"));

        writer
            .write_all(user(None, "same-session-without-id").as_bytes())
            .unwrap();
        let resumed = read(new_cursor).unwrap();
        for record in resumed["records"].as_array().unwrap() {
            assert_eq!(record["identity"]["session_id"], session);
        }
        let resumed = resumed["cursor"].as_str().unwrap();
        writer
            .write_all(user(Some("unexpected-session"), "mismatch").as_bytes())
            .unwrap();
        let error = read(resumed).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Claude session identity changed inside one file"
        );
        drop(writer);
        fs::remove_file(source).unwrap();
    }

    #[test]
    fn prelude_cannot_skip_to_an_arbitrary_record_or_restore_unbound_context_as_a_session() {
        let invalid = [
            PRELUDE.replace("earlier-message", "../../outside"),
            PRELUDE.replace("trackedFileBackups", "unknown"),
            PRELUDE.replace("false", "true"),
            PRELUDE.replace("file-history-snapshot", "unknown-record"),
            PRELUDE.replace(
                "\"isSnapshotUpdate\":false",
                "\"isSnapshotUpdate\":false,\"sessionId\":\"one\",\"sessionId\":\"two\"",
            ),
            PRELUDE.replace(
                "\"isSnapshotUpdate\":false",
                "\"isSnapshotUpdate\":false,\"sessionId\":null",
            ),
        ];
        for raw in invalid {
            assert!(claude::header_identity(raw.as_bytes()).is_err());
        }
        let mut context = claude::header_identity(PRELUDE.as_bytes()).unwrap();
        assert!(
            claude::restore(
                &mut context,
                "p0::c.x.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.1.x.x.x"
            )
            .is_err()
        );
        assert!(claude::restore(&mut context, "c.x.x.x.x.x.x").is_err());
        for raw in [
            user(None, "missing-id"),
            user(Some("../../invalid"), "invalid-id"),
            user(Some("chosen"), "duplicate-id").replace(
                "\"sessionId\":\"chosen\"",
                "\"sessionId\":null,\"sessionId\":\"chosen\"",
            ),
            "{\"type\":\"custom-title\",\"sessionId\":\"title-record\"}\n".into(),
            PRELUDE.into(),
        ] {
            assert!(
                claude::translate_line(raw.as_bytes(), &context, "fixture", PRELUDE.len() as u64)
                    .is_err()
            );
        }
        assert!(
            claude::translate_line(
                user(Some("too-far"), "later").as_bytes(),
                &context,
                "fixture",
                PRELUDE.len() as u64 + 7
            )
            .is_err()
        );
    }

    #[test]
    fn correction_revision_is_bounded_and_gap_identity_is_separate() {
        assert!(claude::revision(0).is_ok());
        assert!(claude::revision((1 << 31) - 1).is_ok());
        assert!(claude::revision(1 << 31).is_err());
    }
}
