//! Bounded, explicitly configured Codex rollout directory reader.
//! One worker owns at most 48 file checkpoints; the receiver commits each
//! file cursor with the evidence batch before acknowledging it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use agenturm_source_process::{Poller, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::CodexFormat;

const MAX_MEMBERS: usize = 48;
const MAX_FILES_PER_POLL: usize = 4;
const MAX_DIRECTORIES: usize = 256;
const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 6;

pub(crate) struct DirectoryPoller;

fn invalid(message: &'static str) -> Box<dyn std::error::Error> {
    message.into()
}

fn member_id(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?;
    let name = relative.to_str().ok_or("non-UTF8 discovered source path")?;
    Ok(format!("{:x}", Sha256::digest(name.as_bytes())))
}

fn files(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    if !root.is_dir() || root.symlink_metadata()?.file_type().is_symlink() {
        return Err(invalid("discovery root must be a real directory"));
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut result = BTreeMap::new();
    let mut directories = 0usize;
    let mut entries = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        directories += 1;
        if directories > MAX_DIRECTORIES {
            return Err(invalid("discovery directory limit exceeded"));
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err(invalid("discovery entry limit exceeded"));
            }
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(invalid("discovery symlink encountered"));
            }
            if kind.is_dir() {
                if depth >= MAX_DEPTH {
                    return Err(invalid("discovery depth limit exceeded"));
                }
                stack.push((path, depth + 1));
            } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                if result.len() >= MAX_MEMBERS {
                    return Err(invalid("discovery member limit exceeded"));
                }
                let id = member_id(root, &path)?;
                if result.insert(id, path).is_some() {
                    return Err(invalid("discovery member collision"));
                }
            }
        }
    }
    Ok(result)
}

fn global_cursor(cursor: &str) -> Result<(u64, String, bool)> {
    if cursor.is_empty() {
        return Ok((0, String::new(), false));
    }
    let mut fields = cursor.split(':');
    let (Some(version @ ("d1" | "d2")), Some(sequence), Some(member), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(invalid("invalid discovery cursor"));
    };
    if sequence.is_empty()
        || sequence.len() > 20
        || !sequence.bytes().all(|b| b.is_ascii_digit())
        || sequence.starts_with('0') && sequence != "0"
        || member.len() != 64
        || !member
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid discovery cursor"));
    }
    Ok((sequence.parse()?, member.to_owned(), version == "d2"))
}

fn next_cursor(sequence: u64, member: &str) -> Result<String> {
    Ok(format!(
        "d1:{}:{}",
        sequence
            .checked_add(1)
            .ok_or("discovery cursor exhausted")?,
        member
    ))
}

fn root_gap_cursor(sequence: u64, last: &str) -> Result<String> {
    Ok(format!(
        "d2:{}:{}",
        sequence
            .checked_add(1)
            .ok_or("discovery cursor exhausted")?,
        if last.is_empty() {
            "0".repeat(64)
        } else {
            last.to_owned()
        }
    ))
}

fn root_gap_batch(source_id: &str, cursor: &str, stamp: u64, max_bytes: usize) -> Result<Value> {
    let evidence = format!("{}:{}:root", source_id, cursor);
    let event_id = format!(
        "codex-discovery-gap:{:x}",
        Sha256::digest(evidence.as_bytes())
    );
    let value = json!({"type":"batch","protocol":1,"source_id":source_id,
                       "root_gap":true,
                       "cursor":cursor,"records":[{"event_id":event_id,"revision":0,
                       "kind":"diagnostic","time_ns":stamp.to_string(),
                       "payload":{"code":"possible_source_gap","severity":"warning"}}]});
    if serde_json::to_vec(&value)?.len() > max_bytes {
        return Err(invalid("discovery batch exceeds frame budget"));
    }
    Ok(value)
}

fn member_batch(
    source_id: &str,
    global: &str,
    member: &str,
    local: &str,
    records: Vec<Value>,
    max_bytes: usize,
) -> Result<Value> {
    let value = json!({"type":"batch","protocol":1,"source_id":source_id,
                       "cursor":global,"records":records,"member_id":member,
                       "member_cursor":local});
    let encoded = serde_json::to_vec(&value)?;
    if encoded.len() > max_bytes
        || agenturm_jsonl_source::structural_tokens(&encoded)
            > agenturm_jsonl_source::MAX_STRUCTURAL_TOKENS
    {
        return Err(invalid("discovery batch exceeds frame budget"));
    }
    Ok(value)
}

fn poll_directory(
    root: &Path,
    source_id: &str,
    cursor: &str,
    members: &BTreeMap<String, String>,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    let (sequence, last, outage) = global_cursor(cursor)?;
    let discovered = match files(root) {
        Ok(files) => files,
        Err(_error) if !outage => {
            let stamp = sequence
                .checked_add(1)
                .ok_or("discovery cursor exhausted")?;
            return root_gap_batch(
                source_id,
                &root_gap_cursor(sequence, &last)?,
                stamp,
                max_bytes,
            );
        }
        Err(error) => return Err(error),
    };
    if members.len() > MAX_MEMBERS {
        return Err(invalid("too many durable discovery members"));
    }
    // Remove missing members first. At the 48-member ceiling, a deleted file
    // and a new file may coexist; admitting the newcomer first would be
    // refused by the host and could stall the source indefinitely.
    if let Some(member) = members.keys().find(|id| !discovered.contains_key(*id)) {
        let stamp = sequence
            .checked_add(1)
            .ok_or("discovery cursor exhausted")?;
        let evidence = format!("{}:{}:{}", source_id, member, cursor);
        let event_id = format!(
            "codex-discovery-gap:{:x}",
            Sha256::digest(evidence.as_bytes())
        );
        return member_batch(
            source_id,
            &next_cursor(sequence, member)?,
            member,
            "",
            vec![json!({"event_id":event_id,"revision":0,"kind":"diagnostic",
                       "time_ns":stamp.to_string(),
                       "payload":{"code":"possible_source_gap","severity":"warning"}})],
            max_bytes,
        );
    }
    let mut order: Vec<_> = discovered.keys().collect();
    let start = order.partition_point(|id| id.as_str() <= last.as_str());
    let count = order.len();
    if count != 0 {
        order.rotate_left(start % count);
    }
    let mut scanned_last = None;
    for member in order.iter().take(MAX_FILES_PER_POLL) {
        scanned_last = Some(member.as_str());
        let old = members.get(member.as_str()).map_or("", String::as_str);
        if let Some(path) = discovered.get(member.as_str()) {
            // Reserve room for the directory envelope and the full local cursor.
            let local_limit = max_bytes
                .checked_sub(1024)
                .ok_or("frame budget too small")?;
            let local = agenturm_jsonl_source::poll_no_follow::<CodexFormat>(
                path,
                source_id,
                old,
                max_records,
                local_limit,
            )?;
            let new = local["cursor"].as_str().ok_or("missing local cursor")?;
            let records = local["records"].as_array().ok_or("missing local records")?;
            if new != old || !records.is_empty() {
                return member_batch(
                    source_id,
                    &next_cursor(sequence, member)?,
                    member,
                    new,
                    records.clone(),
                    max_bytes,
                );
            }
        }
    }
    // Persist the scan position only when not all members were inspected.
    // This bounds prefix verification per poll and resumes fairly on restart.
    let cursor = if order.len() > MAX_FILES_PER_POLL {
        next_cursor(sequence, scanned_last.ok_or("missing scanned member")?)?
    } else if outage {
        format!("d1:{sequence}:{last}")
    } else {
        cursor.to_owned()
    };
    Ok(json!({"type":"batch","protocol":1,"source_id":source_id,
              "cursor":cursor,"records":[]}))
}

impl Poller for DirectoryPoller {
    const PLUGIN_ID: &'static str = <CodexFormat as agenturm_jsonl_source::SourceFormat>::PLUGIN_ID;
    const DISCOVERY: bool = true;
    const ROOT_GAP: bool = true;

    fn poll(
        _path: &Path,
        _source_id: &str,
        _cursor: &str,
        _max_records: usize,
        _max_bytes: usize,
    ) -> Result<Value> {
        Err(invalid("directory reader needs member checkpoints"))
    }

    fn poll_discovery(
        path: &Path,
        source_id: &str,
        cursor: &str,
        members: &BTreeMap<String, String>,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Value> {
        poll_directory(path, source_id, cursor, members, max_records, max_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_emits_once_and_recovery_rechecks_saved_members() {
        let root = std::env::temp_dir().join(format!(
            "agenturm-codex-discovery-root-gap-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("one.jsonl");
        fs::write(
            &file,
            b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"one\"}}\n",
        )
        .unwrap();
        let first = poll_directory(
            &root,
            "source",
            "",
            &BTreeMap::new(),
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        let id = member_id(&root, &file).unwrap();
        let members = BTreeMap::from([(
            id.clone(),
            first["member_cursor"].as_str().unwrap().to_owned(),
        )]);
        let cursor = first["cursor"].as_str().unwrap();
        let moved = root.with_extension("unavailable");
        fs::rename(&root, &moved).unwrap();
        let gap = poll_directory(
            &root,
            "source",
            cursor,
            &members,
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        let replay = poll_directory(
            &root,
            "source",
            cursor,
            &members,
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(gap, replay);
        assert!(gap["cursor"].as_str().unwrap().starts_with("d2:"));
        assert_eq!(gap["root_gap"], true);
        assert_eq!(gap["records"][0]["payload"]["code"], "possible_source_gap");
        assert!(gap.get("member_id").is_none());
        assert!(
            poll_directory(
                &root,
                "source",
                gap["cursor"].as_str().unwrap(),
                &members,
                256,
                agenturm_jsonl_source::max_frame_bytes(),
            )
            .is_err()
        );
        fs::rename(&moved, &root).unwrap();
        let recovered = poll_directory(
            &root,
            "source",
            gap["cursor"].as_str().unwrap(),
            &members,
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        assert!(recovered["cursor"].as_str().unwrap().starts_with("d1:"));
        assert!(recovered["records"].as_array().unwrap().is_empty());
        assert_eq!(members.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn discovers_nested_files_and_rejects_excess() {
        let root =
            std::env::temp_dir().join(format!("agenturm-codex-discovery-{}", std::process::id()));
        fs::create_dir_all(root.join("2026/11")).unwrap();
        fs::write(
            root.join("2026/11/one.jsonl"),
            b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"one\"}}\n",
        )
        .unwrap();
        assert_eq!(files(&root).unwrap().len(), 1);
        for index in 0..48 {
            fs::write(root.join(format!("extra-{index}.jsonl")), b"").unwrap();
        }
        assert!(files(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_checkpoint_set_releases_missing_member_before_new_file() {
        let root = std::env::temp_dir().join(format!(
            "agenturm-codex-discovery-replace-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        for index in 0..48 {
            fs::write(root.join(format!("old-{index:02}.jsonl")), b"").unwrap();
        }
        let old: BTreeMap<_, _> = files(&root)
            .unwrap()
            .into_keys()
            .map(|id| (id, "prior".to_owned()))
            .collect();
        let removed = root.join("old-00.jsonl");
        let removed_id = member_id(&root, &removed).unwrap();
        fs::remove_file(removed).unwrap();
        fs::write(root.join("new.jsonl"), b"").unwrap();
        let batch = poll_directory(
            &root,
            "source",
            "",
            &old,
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        let replay = poll_directory(
            &root,
            "source",
            "",
            &old,
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        assert_eq!(batch, replay);
        assert_eq!(batch["member_id"], removed_id);
        assert_eq!(batch["member_cursor"], "");
        assert_eq!(
            batch["records"][0]["payload"]["code"],
            "possible_source_gap"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_scan_advances_after_four_files() {
        let root = std::env::temp_dir().join(format!(
            "agenturm-codex-discovery-idle-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        for index in 0..5 {
            fs::write(root.join(format!("empty-{index}.jsonl")), b"").unwrap();
        }
        let first = poll_directory(
            &root,
            "source",
            "",
            &BTreeMap::new(),
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        assert!(first.get("member_id").is_none());
        assert!(first["records"].as_array().unwrap().is_empty());
        assert!(first["cursor"].as_str().unwrap().starts_with("d1:1:"));
        let second = poll_directory(
            &root,
            "source",
            first["cursor"].as_str().unwrap(),
            &BTreeMap::new(),
            256,
            agenturm_jsonl_source::max_frame_bytes(),
        )
        .unwrap();
        assert!(second["cursor"].as_str().unwrap().starts_with("d1:2:"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn jsonl_open_refuses_final_component_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "agenturm-codex-discovery-link-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let real = root.join("real.jsonl");
        let link = root.join("link.jsonl");
        fs::write(
            &real,
            b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"one\"}}\n",
        )
        .unwrap();
        symlink(&real, &link).unwrap();
        assert!(
            agenturm_jsonl_source::poll_no_follow::<CodexFormat>(
                &link,
                "source",
                "",
                256,
                agenturm_jsonl_source::max_frame_bytes()
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
