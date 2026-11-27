//! Optional, reference-only nominations for an already selected Pi member.
//! Any failed observational proof omits refs while metadata can still advance.

use agenturm_jsonl_source::{cursor_proof, structural_tokens};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_PREFIX: u64 = 8 * 1024 * 1024;
const MAX_REFS: usize = 48;
const MAX_ROW: usize = 8 * 1024;
const MAX_NOMINATED: usize = 64 * 1024;
const MAX_ENVELOPE: usize = 64 * 1024;
const MAX_COMMAND: usize = 4 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReferenceMode {
    UserText,
    RequestedCommand,
}

impl ReferenceMode {
    fn envelope_key(self) -> &'static str {
        match self {
            Self::UserText => "content_refs",
            Self::RequestedCommand => "command_refs",
        }
    }
}

#[derive(Clone, Copy)]
struct SourceReference<'a> {
    root: &'a Path,
    path: &'a Path,
    member: &'a str,
    prior_cursor: &'a str,
    local: &'a Value,
    max_bytes: usize,
}

struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = Unique;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Unique, E> {
                Ok(Unique(Value::Number(Number::from(value))))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Unique, E> {
                Ok(Unique(Value::Number(Number::from(value))))
            }
            fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Unique, E> {
                // serde_json may coerce an integer beyond the receiver's
                // i64..u64 range into f64. Withhold on every float.
                Err(E::custom("unproven Pi number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Unique, E> {
                Ok(Unique(Value::String(value.to_owned())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Unique, E> {
                Ok(Unique(Value::String(value)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Unique, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<Unique>()? {
                    values.push(value.0);
                }
                Ok(Unique(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, Unique>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(de::Error::custom("duplicate Pi key"));
                    }
                }
                Ok(Unique(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

fn sha(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}

fn same_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) == (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    )
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn host_timestamp_compatible(value: &str) -> bool {
    let raw = value.as_bytes();
    if raw.len() < 20
        || raw[4] != b'-'
        || raw[7] != b'-'
        || raw[10] != b'T'
        || raw[13] != b':'
        || raw[16] != b':'
        || [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .iter()
            .any(|index| !raw[*index].is_ascii_digit())
        || raw[17] > b'5'
    {
        return false;
    }
    let mut position = 19;
    if raw.get(position) == Some(&b'.') {
        position += 1;
        let start = position;
        while raw.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if !(1..=9).contains(&(position - start)) {
            return false;
        }
    }
    if raw.get(position) == Some(&b'Z') {
        return position + 1 == raw.len();
    }
    if !matches!(raw.get(position), Some(b'+' | b'-')) || position + 6 != raw.len() {
        return false;
    }
    raw[position + 1].is_ascii_digit()
        && raw[position + 2].is_ascii_digit()
        && raw[position + 3] == b':'
        && raw[position + 4].is_ascii_digit()
        && raw[position + 5].is_ascii_digit()
}

fn member_path(root: &Path, path: &Path, member: &str) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?.to_str()?;
    let parts: Vec<_> = relative.split('/').collect();
    if relative.is_empty()
        || relative.len() > 1024
        || relative.contains('\\')
        || relative.chars().any(|character| character.is_control())
        || parts.len() > 7
        || parts.iter().any(|part| matches!(*part, "" | "." | ".."))
        || !parts.last()?.ends_with(".jsonl")
        || sha(relative.as_bytes()) != member
    {
        return None;
    }
    // The receiver independently walks the path with openat/O_NOFOLLOW.
    // This observation only avoids nominating obviously switched paths.
    if root.ancestors().any(|part| {
        part.symlink_metadata()
            .ok()
            .is_none_or(|metadata| metadata.file_type().is_symlink())
    }) {
        return None;
    }
    let mut component = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        component.push(part);
        let metadata = component.symlink_metadata().ok()?;
        if metadata.file_type().is_symlink()
            || (index + 1 == parts.len() && !metadata.is_file())
            || (index + 1 < parts.len() && !metadata.is_dir())
        {
            return None;
        }
    }
    Some(relative.to_owned())
}

fn nominated(
    source: SourceReference<'_>,
    batch: &Value,
    mode: ReferenceMode,
) -> Option<Vec<Value>> {
    let SourceReference {
        root,
        path,
        member,
        prior_cursor,
        local,
        max_bytes,
    } = source;
    let source_id = batch["source_id"].as_str()?;
    let relative = member_path(root, path, member)?;
    let current = cursor_proof(local["cursor"].as_str()?).ok()??;
    if current.version != 2
        || current.offset > MAX_PREFIX
        || current.device > i64::MAX as u64
        || current.inode > i64::MAX as u64
    {
        return None;
    }
    let prior = cursor_proof(prior_cursor).ok()?;
    if local["records"].as_array()?.iter().any(|record| {
        record["kind"] == "diagnostic" && record["payload"]["code"] == "possible_source_gap"
    }) {
        return None;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let before = file.metadata().ok()?;
    if !before.is_file()
        || before.dev() != current.device
        || before.ino() != current.inode
        || current.offset > before.len()
    {
        return None;
    }
    let mut prefix = Vec::with_capacity(usize::try_from(current.offset).ok()?);
    (&mut file)
        .take(current.offset)
        .read_to_end(&mut prefix)
        .ok()?;
    if prefix.len() != usize::try_from(current.offset).ok()? || sha(&prefix) != current.digest {
        return None;
    }
    let header_end = prefix.iter().position(|byte| *byte == b'\n')? + 1;
    // The receiver only searches its first MAX_ROW_BYTES for the newline.
    // A valid metadata header beyond that bound cannot carry content refs.
    if header_end > MAX_ROW {
        return None;
    }
    let header = serde_json::from_slice::<Unique>(&prefix[..header_end])
        .ok()?
        .0;
    if header["type"] != "session"
        || sha(&prefix[..header_end]) != current.header_digest
        || !identifier(header["id"].as_str()?)
    {
        return None;
    }
    let session_id = match header.get("sessionId") {
        Some(value) => value.as_str().filter(|value| identifier(value))?,
        None => header["id"].as_str()?,
    };
    let worker_id = match header.get("agentId") {
        Some(value) => value.as_str()?,
        None => "",
    };
    if !worker_id.is_empty() && !identifier(worker_id) {
        return None;
    }
    let canonical = sha(format!(
        "{}:{}:{}",
        before.dev(),
        before.ino(),
        current.header_digest
    )
    .as_bytes());
    if current.generation != canonical {
        return None;
    }
    let start = if let Some(prior) = prior {
        if prior.version != 2
            || (prior.device, prior.inode) != (current.device, current.inode)
            || prior.header_digest != current.header_digest
            || prior.generation != canonical
            || prior.offset > current.offset
            || sha(&prefix[..usize::try_from(prior.offset).ok()?]) != prior.digest
        {
            return None;
        }
        usize::try_from(prior.offset).ok()?
    } else {
        0
    };
    if start > 0 && prefix.get(start - 1) != Some(&b'\n') {
        return None;
    }
    let after = file.metadata().ok()?;
    if !same_file(&before, &after) {
        return None;
    }
    let records = local["records"].as_array()?;
    let base = serde_json::to_vec(batch).ok()?;
    let base_len = base.len();
    let base_tokens = structural_tokens(&base);
    let mut refs = Vec::new();
    let mut bytes = 0usize;
    let mut offset = if mode == ReferenceMode::RequestedCommand {
        header_end
    } else {
        start
    };
    let mut last_user = String::new();
    while offset < prefix.len() {
        let end = offset + prefix[offset..].iter().position(|byte| *byte == b'\n')? + 1;
        let raw = &prefix[offset..end];
        // Command provenance depends on the most recent user row even when
        // that row preceded the current durable member checkpoint.
        if mode == ReferenceMode::RequestedCommand {
            if raw.len() > MAX_ROW {
                return None;
            }
            let Unique(row) = serde_json::from_slice::<Unique>(raw).ok()?;
            if row["type"] == "message" && row["message"]["role"] == "user" {
                last_user = row["id"].as_str().filter(|id| identifier(id))?.to_owned();
            }
            if offset < start {
                offset = end;
                continue;
            }
        }
        if raw.len() <= MAX_ROW
            && bytes + raw.len() <= MAX_NOMINATED
            && refs.len() < MAX_REFS
            && let Ok(Unique(row)) = serde_json::from_slice::<Unique>(raw)
        {
            let row_id = row["id"].as_str();
            if mode == ReferenceMode::UserText
                && row["type"] == "message"
                && row["message"]["role"] == "user"
                && row["timestamp"]
                    .as_str()
                    .is_some_and(host_timestamp_compatible)
                && let Some(row_id) = row_id.filter(|id| identifier(id))
            {
                let event_id = format!("pi:{canonical}:{row_id}:message");
                if let Some(record) = records.iter().find(|record| {
                    record["event_id"] == event_id
                        && record["revision"] == 0
                        && record["kind"] == "message"
                        && record["payload"] == json!({"role":"user"})
                        && record["identity"]["turn_id"] == row_id
                }) && let Some(session) = record["identity"]["session_id"].as_str()
                    && session == session_id
                {
                    let reference = json!({
                        "schema":1,"source_id":source_id,"member_id":member,
                        "relative_member_path":relative,"row_offset":offset,
                        "row_length":raw.len(),"row_sha256":sha(raw),
                        "event_id":event_id,"revision":0,
                        "native_session_id":session,"native_turn_id":row_id,
                        "kind":"pi.user_text"
                    });
                    refs.push(reference);
                    let envelope = json!({"schema_version":1,"refs":refs});
                    let encoded = serde_json::to_vec(&envelope).ok()?;
                    if encoded.len() > MAX_ENVELOPE
                        || base_len + encoded.len() + b",\"content_refs\":".len() > max_bytes
                        || base_tokens + structural_tokens(&encoded) + 3
                            > agenturm_jsonl_source::MAX_STRUCTURAL_TOKENS
                    {
                        refs.pop();
                    } else {
                        bytes += raw.len();
                    }
                }
            }
            if mode == ReferenceMode::RequestedCommand
                && row["type"] == "message"
                && row["message"]["role"] == "assistant"
                && !last_user.is_empty()
                && row["timestamp"]
                    .as_str()
                    .is_some_and(host_timestamp_compatible)
                && let Some(row_id) = row_id.filter(|id| identifier(id))
                && let Some(blocks) = row["message"]["content"].as_array()
                && blocks.len() <= 128
            {
                for (index, block) in blocks.iter().enumerate() {
                    if block["type"] != "toolCall" || block["name"] != "bash" {
                        continue;
                    }
                    let Some(call_id) = block["id"].as_str().filter(|id| identifier(id)) else {
                        continue;
                    };
                    let Some(command) = block["arguments"]["command"].as_str() else {
                        continue;
                    };
                    if command.is_empty() || command.len() > MAX_COMMAND {
                        continue;
                    }
                    let event_id = format!("pi:{canonical}:{row_id}:tool-{index}");
                    let present = records.iter().any(|record| {
                        record["event_id"] == event_id
                            && record["revision"] == 0
                            && record["kind"] == "tool"
                            && record["payload"] == json!({"category":"shell","result":"started"})
                            && record["identity"]["session_id"] == session_id
                            && record["identity"]["turn_id"] == last_user.as_str()
                            && record["identity"]["operation_id"] == call_id
                            && record["identity"]["worker_id"].as_str().unwrap_or("") == worker_id
                    });
                    if !present {
                        continue;
                    }
                    refs.push(json!({
                        "schema":1,"source_id":source_id,"member_id":member,
                        "relative_member_path":relative,"row_offset":offset,
                        "row_length":raw.len(),"row_sha256":sha(raw),
                        "event_id":event_id,"revision":0,
                        "native_session_id":session_id,"native_turn_id":last_user,
                        "assistant_row_id":row_id,"block_index":index,
                        "tool_call_id":call_id,"kind":"pi.shell_command_text"
                    }));
                    let envelope = json!({"schema_version":1,"refs":refs});
                    let encoded = serde_json::to_vec(&envelope).ok()?;
                    if encoded.len() > MAX_ENVELOPE
                        || base_len + encoded.len() + b",\"command_refs\":".len() > max_bytes
                        || base_tokens + structural_tokens(&encoded) + 3
                            > agenturm_jsonl_source::MAX_STRUCTURAL_TOKENS
                    {
                        refs.pop();
                    } else {
                        bytes += raw.len();
                    }
                    if refs.len() >= MAX_REFS || bytes >= MAX_NOMINATED {
                        break;
                    }
                }
            }
        }
        offset = end;
    }
    Some(refs)
}

fn decorate_mode(source: SourceReference<'_>, batch: &mut Value, mode: ReferenceMode) {
    let refs = nominated(source, batch, mode).unwrap_or_default();
    batch[mode.envelope_key()] = json!({"schema_version":1,"refs":refs});
    // Check the actual final serialization once. Any unexpected expansion
    // withholds refs, preserving the metadata/cursor batch.
    let valid = serde_json::to_vec(batch).ok().is_some_and(|encoded| {
        encoded.len() <= source.max_bytes
            && structural_tokens(&encoded) <= agenturm_jsonl_source::MAX_STRUCTURAL_TOKENS
    });
    if !valid {
        batch[mode.envelope_key()] = json!({"schema_version":1,"refs":[]});
    }
}

pub(crate) fn decorate(
    root: &Path,
    path: &Path,
    member: &str,
    prior_cursor: &str,
    local: &Value,
    batch: &mut Value,
    max_bytes: usize,
) {
    decorate_mode(
        SourceReference {
            root,
            path,
            member,
            prior_cursor,
            local,
            max_bytes,
        },
        batch,
        ReferenceMode::UserText,
    );
}

pub(crate) fn decorate_command(
    root: &Path,
    path: &Path,
    member: &str,
    prior_cursor: &str,
    local: &Value,
    batch: &mut Value,
    max_bytes: usize,
) {
    decorate_mode(
        SourceReference {
            root,
            path,
            member,
            prior_cursor,
            local,
            max_bytes,
        },
        batch,
        ReferenceMode::RequestedCommand,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PiFormat;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agenturm-pi-content-emitter-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn file(&self, relative: &str, user_count: usize) -> std::path::PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut raw = b"{\"type\":\"session\",\"id\":\"pi-one\"}\n".to_vec();
            for index in 0..user_count {
                raw.extend_from_slice(
                    format!(
                        "{{\"type\":\"message\",\"id\":\"user-{index}\",\"timestamp\":\"2026-10-24T12:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"private-{index}\"}}]}}}}\n"
                    )
                    .as_bytes(),
                );
            }
            fs::write(&path, raw).unwrap();
            path
        }

        fn poll(&self, path: &Path, relative: &str) -> (Value, Value, String) {
            let local = agenturm_jsonl_source::poll_no_follow::<PiFormat<true>>(
                path,
                "source",
                "",
                256,
                128 * 1024,
            )
            .unwrap();
            let member = sha(relative.as_bytes());
            let batch = json!({"type":"batch","protocol":1,"source_id":"source",
                "cursor":format!("d1:1:{member}"),"member_id":member,
                "member_cursor":local["cursor"],"records":local["records"]});
            (local, batch, member)
        }

        fn append_command(&self, path: &Path, name: &str, command: &str) {
            use std::io::Write;
            let row = json!({"type":"message","id":"assistant-1",
                "timestamp":"2026-10-24T12:00:01Z","message":{"role":"assistant",
                "content":[{"type":"toolCall","id":"call-1","name":name,
                            "arguments":{"command":command}}]}});
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{row}").unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_unicode_member_nominates_only_references() {
        let scratch = Scratch::new();
        let relative = "équipe/one.jsonl";
        let path = scratch.file(relative, 1);
        let (local, mut batch, member) = scratch.poll(&path, relative);
        let records = batch["records"].clone();
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert_eq!(batch["records"], records);
        let refs = batch["content_refs"]["refs"].as_array().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["relative_member_path"], relative);
        assert_eq!(
            refs[0]["event_id"],
            records
                .as_array()
                .unwrap()
                .iter()
                .find(|record| record["kind"] == "message")
                .unwrap()["event_id"]
        );
        assert!(!batch.to_string().contains("private-0"));
        assert_eq!(refs[0]["row_sha256"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn requested_command_nominates_exact_tool_without_text() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 1);
        scratch.append_command(&path, "bash", "password=private-value");
        let (local, mut batch, member) = scratch.poll(&path, relative);
        let records = batch["records"].clone();
        decorate_command(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert_eq!(batch["records"], records);
        let refs = batch["command_refs"]["refs"].as_array().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["kind"], "pi.shell_command_text");
        assert_eq!(refs[0]["native_turn_id"], "user-0");
        assert_eq!(refs[0]["tool_call_id"], "call-1");
        assert_eq!(refs[0]["block_index"], 0);
        assert!(!batch.to_string().contains("private-value"));
    }

    #[test]
    fn requested_command_uses_preceding_acknowledged_user_turn() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 1);
        let (first, _, _) = scratch.poll(&path, relative);
        let previous = first["cursor"].as_str().unwrap().to_owned();
        scratch.append_command(&path, "bash", "echo safe");
        let local = agenturm_jsonl_source::poll_no_follow::<PiFormat<false, true>>(
            &path,
            "source",
            &previous,
            256,
            128 * 1024,
        )
        .unwrap();
        let member = sha(relative.as_bytes());
        let mut batch = json!({"type":"batch","protocol":1,"source_id":"source",
            "cursor":format!("d1:2:{member}"),"member_id":member,
            "member_cursor":local["cursor"],"records":local["records"]});
        decorate_command(
            &scratch.0,
            &path,
            &member,
            &previous,
            &local,
            &mut batch,
            128 * 1024,
        );
        let refs = batch["command_refs"]["refs"].as_array().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["native_turn_id"], "user-0");
    }

    #[test]
    fn requested_command_withholds_non_bash_or_changed_source() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 1);
        scratch.append_command(&path, "read", "echo safe");
        let (local, mut batch, member) = scratch.poll(&path, relative);
        decorate_command(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert!(batch["command_refs"]["refs"].as_array().unwrap().is_empty());
        scratch.append_command(&path, "bash", "echo safe");
        let mut batch = json!({"type":"batch","protocol":1,"source_id":"source",
            "cursor":format!("d1:1:{member}"),"member_id":member,
            "member_cursor":local["cursor"],"records":local["records"]});
        decorate_command(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert!(batch["command_refs"]["refs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn reference_budget_omits_without_losing_metadata() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 60);
        let (local, mut batch, member) = scratch.poll(&path, relative);
        let records = batch["records"].clone();
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert_eq!(batch["records"], records);
        assert_eq!(batch["content_refs"]["refs"].as_array().unwrap().len(), 48);

        let mut narrow = json!({"type":"batch","protocol":1,"source_id":"source",
            "cursor":format!("d1:1:{member}"),"member_id":member,
            "member_cursor":local["cursor"],"records":local["records"]});
        let metadata_len = serde_json::to_vec(&narrow).unwrap().len();
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut narrow,
            metadata_len + 128,
        );
        assert_eq!(narrow["records"], records);
        assert!(
            narrow["content_refs"]["refs"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn changed_or_incompatible_path_withholds_references() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 1);
        let (local, mut batch, member) = scratch.poll(&path, relative);
        fs::write(&path, b"changed\n").unwrap();
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert!(batch["content_refs"]["refs"].as_array().unwrap().is_empty());

        let path = scratch.file("project/bad\\name.jsonl", 1);
        let (local, mut batch, member) = scratch.poll(&path, "project/bad\\name.jsonl");
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert!(batch["content_refs"]["refs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn receiver_file_identity_bound_withholds_references() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 1);
        let (mut local, mut batch, member) = scratch.poll(&path, relative);
        let cursor = local["cursor"].as_str().unwrap();
        let mut fields: Vec<_> = cursor.split(':').map(str::to_owned).collect();
        fields[1] = (i64::MAX as u64 + 1).to_string();
        local["cursor"] = json!(fields.join(":"));
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert!(batch["content_refs"]["refs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn duplicate_keys_are_metadata_only() {
        let scratch = Scratch::new();
        let relative = "project/one.jsonl";
        let path = scratch.file(relative, 0);
        let mut raw = fs::read(&path).unwrap();
        raw.extend_from_slice(b"{\"type\":\"message\",\"id\":\"user-1\",\"timestamp\":\"2026-10-24T12:00:00Z\",\"message\":{\"role\":\"user\",\"role\":\"user\"}}\n");
        fs::write(&path, raw).unwrap();
        let (local, mut batch, member) = scratch.poll(&path, relative);
        decorate(
            &scratch.0,
            &path,
            &member,
            "",
            &local,
            &mut batch,
            128 * 1024,
        );
        assert!(!batch["records"].as_array().unwrap().is_empty());
        assert!(batch["content_refs"]["refs"].as_array().unwrap().is_empty());
    }
}
