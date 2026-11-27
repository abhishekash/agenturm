//! Read-only roster labels from exact, configured native source receipts.

use agenturm_jsonl_source::{CursorProof, cursor_proof};
use agenturm_kernel::ExtensionId;
use agenturm_store::{SessionPage, Store};
use rustix::fs::{Mode, OFlags, open, openat};
use serde::{Deserialize, Deserializer, de::IgnoredAny};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use super::{NativeContext, SessionKey};
use crate::discovery::{AuthorizedSource, SourceAuthority};
use unicode_segmentation::UnicodeSegmentation;

const CODEX: &str = "example.rust-codex-harness";
const CLAUDE: &str = "example.rust-claude-harness";
const MAX_HEADER_BYTES: usize = 256 * 1024;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_CLAUDE_PRELUDE_BYTES: usize = 64 * 1024;
const MAX_CLAUDE_PREFIX: u64 = 16 * 1024 * 1024;
const MAX_CLAUDE_ROSTER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INDEX_RECORD_BYTES: usize = 4096;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn load(
    store: Option<&Store>,
    history: &Path,
    page: &SessionPage,
    authority: Option<&SourceAuthority>,
) -> BTreeMap<SessionKey, NativeContext> {
    let (Some(store), Some(authority)) = (store, authority) else {
        return BTreeMap::new();
    };
    if page.sessions.is_empty() {
        return BTreeMap::new();
    }
    let Some(result) = load_bound(store, history, page, authority) else {
        return BTreeMap::new();
    };
    result
}

fn load_bound(
    store: &Store,
    history: &Path,
    page: &SessionPage,
    authority: &SourceAuthority,
) -> Option<BTreeMap<SessionKey, NativeContext>> {
    let identity = store.history_identity().ok()?;
    store
        .verify_native_context_snapshot(page.snapshot_id)
        .ok()?;
    let current = Store::open_read_only(history).ok()?;
    if current.history_identity().ok()? != identity {
        return None;
    }
    current
        .verify_native_context_snapshot(page.snapshot_id)
        .ok()?;
    let keys: BTreeSet<_> = page
        .sessions
        .iter()
        .map(|row| (row.harness_id.clone(), row.session_id.clone()))
        .collect();
    let sources = page
        .sessions
        .iter()
        .map(|row| row.harness_id.as_str())
        .collect::<BTreeSet<_>>();
    let recovered = authority.follow.as_ref().map_or_else(Vec::new, |follow| {
        let current = authority
            .sources
            .iter()
            .map(|source| source.source_id.as_str())
            .collect::<BTreeSet<_>>();
        let wanted = sources
            .difference(&current)
            .map(|source| (*source).to_owned())
            .collect();
        follow
            .authorize_retained_sources(&wanted)
            .unwrap_or_default()
    });
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut scopes = BTreeMap::<String, usize>::new();
    let mut claude_budget = MAX_CLAUDE_ROSTER_BYTES;
    for source in authority.sources.iter().rev().chain(&recovered) {
        if !seen.insert(source.source_id.as_str())
            || !matches!(source.plugin_id.as_str(), CODEX | CLAUDE)
            || (source.plugin_id == CLAUDE && !sources.contains(source.source_id.as_str()))
        {
            continue;
        }
        let Some(candidate) = candidate(&current, source, &keys, &mut claude_budget) else {
            continue;
        };
        if candidate.plugin == CODEX {
            *scopes.entry(candidate.key.1.clone()).or_default() += 1;
        }
        if keys.contains(&candidate.key) {
            candidates.push(candidate);
        }
    }

    let allowed_home = authority.codex_home.as_ref().filter(|home| {
        candidates.iter().any(|candidate| {
            candidate.plugin == CODEX && candidate.path.starts_with(home.join("sessions"))
        })
    });
    let ids = candidates
        .iter()
        .filter(|candidate| {
            candidate.plugin == CODEX
                && allowed_home
                    .is_some_and(|home| candidate.path.starts_with(home.join("sessions")))
        })
        .map(|candidate| candidate.key.1.clone())
        .collect();
    let state = allowed_home.map_or_else(BTreeMap::new, |home| read_codex_state(home, &ids));
    let index = allowed_home.map_or_else(BTreeMap::new, |home| read_codex_index(home, &ids));

    let current = Store::open_read_only(history).ok()?;
    if current.history_identity().ok()? != identity {
        return None;
    }
    current
        .verify_native_context_snapshot(page.snapshot_id)
        .ok()?;
    let mut result = BTreeMap::new();
    for mut candidate in candidates {
        if !still_bound(&current, &candidate) {
            continue;
        }
        let session = &candidate.key.1;
        if candidate.plugin == CODEX
            && allowed_home.is_some_and(|home| candidate.path.starts_with(home.join("sessions")))
        {
            let metadata = state.get(session);
            let exact = metadata.and_then(|row| {
                row.rollout_path
                    .as_deref()
                    .is_some_and(|path| Path::new(path) == candidate.path)
                    .then_some(row)
            });
            if let Some(row) = exact {
                candidate.metadata.title =
                    label(row.name.as_deref()).or_else(|| label(row.title.as_deref()));
                candidate.metadata.agent_name =
                    label(row.agent_nickname.as_deref()).or(candidate.metadata.agent_name);
                candidate.metadata.agent_role =
                    label(row.agent_role.as_deref()).or(candidate.metadata.agent_role);
                candidate.metadata.agent_path =
                    label(row.agent_path.as_deref()).or(candidate.metadata.agent_path);
                candidate.metadata.model_provider =
                    label(row.model_provider.as_deref()).or(candidate.metadata.model_provider);
                candidate.metadata.workspace =
                    workspace(row.cwd.as_deref()).or(candidate.metadata.workspace);
            }
            // The index supplies ID only. Reject a duplicate configured
            // thread or a conflicting live state row with a different path.
            if scopes.get(session) == Some(&1)
                && (metadata.is_none() || exact.is_some())
                && let Some((updated, title)) = index.get(session)
                && (candidate.metadata.title.is_none()
                    || exact
                        .and_then(|row| row.updated_at_ms)
                        .is_some_and(|state| *updated > state))
            {
                candidate.metadata.title = Some(title.clone());
            }
        }
        result.insert(candidate.key, candidate.metadata);
    }
    Some(result)
}

struct Candidate<'a> {
    key: SessionKey,
    plugin: &'a str,
    path: PathBuf,
    cursor: String,
    proof: CursorProof,
    stamp: Metadata,
    metadata: NativeContext,
}

fn candidate<'a>(
    store: &Store,
    source: &'a AuthorizedSource,
    keys: &BTreeSet<SessionKey>,
    claude_budget: &mut u64,
) -> Option<Candidate<'a>> {
    let path = &source.source_root;
    if path.extension()? != "jsonl" {
        return None;
    }
    let id = ExtensionId::parse(&source.source_id).ok()?;
    let cursor = store.source_cursor(&source.source_id).ok()??;
    let proof = cursor_proof(&cursor).ok()??;
    let mut file = open_member(path)?;
    let stamp = file.metadata().ok()?;
    if proof.version != 2
        || proof.device != stamp.dev()
        || proof.inode != stamp.ino()
        || proof.offset == 0
        || proof.offset > stamp.len()
    {
        return None;
    }
    let (session, metadata) = if source.plugin_id == CODEX {
        codex_header(&mut file, &proof)?
    } else {
        claude_metadata(&mut file, &proof, &id, keys, claude_budget)?
    };
    if source.plugin_id == CLAUDE && !same_stamp(&file, &stamp) {
        return None;
    }
    Some(Candidate {
        key: (id, session),
        plugin: &source.plugin_id,
        path: path.clone(),
        cursor,
        proof,
        stamp,
        metadata,
    })
}

fn still_bound(store: &Store, candidate: &Candidate<'_>) -> bool {
    let Ok(Some(cursor)) = store.source_cursor(candidate.key.0.as_str()) else {
        return false;
    };
    let Some(mut file) = open_member(&candidate.path) else {
        return false;
    };
    if candidate.plugin == CLAUDE {
        return cursor == candidate.cursor && same_stamp(&file, &candidate.stamp);
    }
    let Ok(Some(current)) = cursor_proof(&cursor) else {
        return false;
    };
    let prior = &candidate.proof;
    if current.version != 2
        || current.device != prior.device
        || current.inode != prior.inode
        || current.generation != prior.generation
        || current.header_digest != prior.header_digest
        || current.offset < prior.offset
    {
        return false;
    }
    // Codex only consumes the authenticated header. Allow an ordinary append
    // during the SQLite/index lookup while checking identity and header again.
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    metadata.dev() == current.device
        && metadata.ino() == current.inode
        && current.offset <= metadata.len()
        && read_verified_header(&mut file, &current).is_some()
}

fn open_member(path: &Path) -> Option<File> {
    let mut parts = path.components();
    if parts.next()? != Component::RootDir {
        return None;
    }
    let directories = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", directories, Mode::empty()).ok()?;
    let mut parts = parts.peekable();
    while let Some(Component::Normal(name)) = parts.next() {
        if parts.peek().is_none() {
            let fd = openat(
                &directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .ok()?;
            let file = File::from(fd);
            let metadata = file.metadata().ok()?;
            return (metadata.is_file() && metadata.nlink() == 1).then_some(file);
        }
        directory = openat(&directory, name, directories, Mode::empty()).ok()?;
    }
    None
}

fn same_stamp(file: &File, expected: &Metadata) -> bool {
    file.metadata().is_ok_and(|actual| {
        actual.dev() == expected.dev()
            && actual.ino() == expected.ino()
            && actual.len() == expected.len()
            && (actual.mtime(), actual.mtime_nsec()) == (expected.mtime(), expected.mtime_nsec())
            && (actual.ctime(), actual.ctime_nsec()) == (expected.ctime(), expected.ctime_nsec())
    })
}

fn next_line(reader: &mut impl BufRead, line: &mut Vec<u8>, limit: usize) -> Option<usize> {
    line.clear();
    let count = reader
        .take((limit + 1) as u64)
        .read_until(b'\n', line)
        .ok()?;
    (count <= limit && (count == 0 || line.last() == Some(&b'\n'))).then_some(count)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Deserialize)]
struct CodexHeader {
    #[serde(rename = "type")]
    kind: String,
    payload: CodexHeaderFields,
}

#[derive(Deserialize)]
struct CodexHeaderFields {
    id: String,
    cwd: Option<String>,
    model_provider: Option<String>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    agent_path: Option<String>,
}

fn read_verified_header(file: &mut File, proof: &CursorProof) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut bounded = BufReader::new(file.take(proof.offset));
    if next_line(&mut bounded, &mut line, MAX_HEADER_BYTES)? == 0
        || hash(&line) != proof.header_digest
    {
        return None;
    }
    Some(line)
}

fn codex_header(file: &mut File, proof: &CursorProof) -> Option<(String, NativeContext)> {
    let line = read_verified_header(file, proof)?;
    let header = serde_json::from_slice::<CodexHeader>(&line).ok()?;
    let row = header.payload;
    (header.kind == "session_meta").then(|| {
        (
            row.id,
            NativeContext {
                agent_name: label(row.agent_nickname.as_deref()),
                agent_role: label(row.agent_role.as_deref()),
                agent_path: label(row.agent_path.as_deref()),
                model_provider: label(row.model_provider.as_deref()),
                workspace: workspace(row.cwd.as_deref()),
                ..NativeContext::default()
            },
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRow {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "present_claude_identifier")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "present_claude_identifier")]
    agent_id: Option<String>,
    custom_title: Option<String>,
    agent_name: Option<String>,
    cwd: Option<String>,
}

fn present_claude_identifier<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeFileHistoryPrelude {
    message_id: String,
    snapshot: ClaudeFileHistorySnapshot,
    is_snapshot_update: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeFileHistorySnapshot {
    message_id: String,
    #[serde(rename = "trackedFileBackups")]
    _backups: BTreeMap<String, IgnoredAny>,
}

fn claude_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn claude_metadata(
    file: &mut File,
    proof: &CursorProof,
    source: &ExtensionId,
    keys: &BTreeSet<SessionKey>,
    budget: &mut u64,
) -> Option<(String, NativeContext)> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut reader = BufReader::new(file.take(proof.offset));
    let mut line = Vec::new();
    let length = next_line(&mut reader, &mut line, MAX_RECORD_BYTES)?;
    if length == 0 || hash(&line) != proof.header_digest {
        return None;
    }
    let mut first = serde_json::from_slice::<ClaudeRow>(&line).ok()?;
    let from_prelude = first.session_id.is_none();
    let within_budget = proof.offset <= MAX_CLAUDE_PREFIX && proof.offset <= *budget;
    let mut digest = Sha256::new();
    digest.update(&line);
    let mut count = length as u64;
    if from_prelude {
        if !within_budget
            || length > MAX_CLAUDE_PRELUDE_BYTES
            || first.kind.as_deref() != Some("file-history-snapshot")
            || first.agent_id.is_some()
        {
            return None;
        }
        let prelude: ClaudeFileHistoryPrelude = serde_json::from_slice(&line).ok()?;
        if prelude.is_snapshot_update
            || !claude_identifier(&prelude.message_id)
            || prelude.message_id != prelude.snapshot.message_id
        {
            return None;
        }
        let next = next_line(&mut reader, &mut line, MAX_RECORD_BYTES)?;
        first = serde_json::from_slice(&line).ok()?;
        if first.kind.as_deref() != Some("user")
            || !first.session_id.as_deref().is_some_and(claude_identifier)
            || first
                .agent_id
                .as_deref()
                .is_some_and(|id| !claude_identifier(id))
        {
            return None;
        }
        digest.update(&line);
        count += next as u64;
    }
    let session = first.session_id.as_ref()?.clone();
    if !keys.contains(&(source.clone(), session.clone())) {
        return None;
    }

    let header_workspace = workspace(first.cwd.as_deref());
    // Only an identity on the first physical line is proved by the header
    // digest alone. A prelude-bound identity always requires the full prefix.
    let header_only = || {
        (!from_prelude).then(|| {
            (
                session.clone(),
                NativeContext {
                    workspace: header_workspace.clone(),
                    partial_title: true,
                    ..NativeContext::default()
                },
            )
        })
    };
    if !within_budget {
        return header_only();
    }
    *budget -= proof.offset;
    let agent = first.agent_id.clone();
    let mut metadata = NativeContext::default();
    update_claude(&mut metadata, first, &session, agent.as_deref());
    loop {
        let bytes = match next_line(&mut reader, &mut line, MAX_RECORD_BYTES) {
            Some(bytes) => bytes,
            None if line.len() > MAX_RECORD_BYTES => return header_only(),
            None => return None,
        };
        if bytes == 0 {
            break;
        }
        count += bytes as u64;
        digest.update(&line);
        if let Ok(row) = serde_json::from_slice(&line) {
            update_claude(&mut metadata, row, &session, agent.as_deref());
        }
    }
    (count == proof.offset && format!("{:x}", digest.finalize()) == proof.digest)
        .then_some((session, metadata))
}

fn update_claude(metadata: &mut NativeContext, row: ClaudeRow, session: &str, agent: Option<&str>) {
    if row.session_id.as_deref() != Some(session) || row.agent_id.as_deref() != agent {
        return;
    }
    match row.kind.as_deref() {
        Some("custom-title") => metadata.title = label(row.custom_title.as_deref()),
        Some("agent-name") => metadata.agent_name = label(row.agent_name.as_deref()),
        _ => {}
    }
    if let Some(path) = workspace(row.cwd.as_deref()) {
        metadata.workspace = Some(path);
    }
}

fn read_codex_state(
    home: &Path,
    ids: &BTreeSet<String>,
) -> BTreeMap<String, agenturm_store::CodexThreadMetadata> {
    let db = home.join("state_5.sqlite");
    let Some(before) = open_member(&db).and_then(|file| file.metadata().ok()) else {
        return BTreeMap::new();
    };
    if before.len() > MAX_STATE_BYTES {
        return BTreeMap::new();
    }
    // Native SQLite chooses its own WAL companions. Refuse unexpected
    // objects at those exact paths, and limit the files it can map.
    for name in [
        "state_5.sqlite-wal",
        "state_5.sqlite-shm",
        "state_5.sqlite-journal",
    ] {
        let path = home.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && (!metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_STATE_BYTES)
        {
            return BTreeMap::new();
        }
    }
    let rows = agenturm_store::read_codex_thread_metadata(&db, ids).unwrap_or_default();
    let same_file = open_member(&db)
        .and_then(|file| file.metadata().ok())
        .is_some_and(|after| after.dev() == before.dev() && after.ino() == before.ino());
    if same_file { rows } else { BTreeMap::new() }
}

#[derive(Deserialize)]
struct IndexRow {
    id: String,
    thread_name: Option<String>,
    updated_at: String,
}

fn read_codex_index(home: &Path, ids: &BTreeSet<String>) -> BTreeMap<String, (i64, String)> {
    let Some(rows) = read_index(home, ids) else {
        return BTreeMap::new();
    };
    rows
}

fn read_index(home: &Path, ids: &BTreeSet<String>) -> Option<BTreeMap<String, (i64, String)>> {
    let path = home.join("session_index.jsonl");
    let mut file = open_member(&path)?;
    let stamp = file.metadata().ok()?;
    if stamp.len() > MAX_INDEX_BYTES {
        return None;
    }
    let mut reader = BufReader::new((&mut file).take(stamp.len()));
    let mut line = Vec::new();
    let mut names = BTreeMap::<_, (i64, Option<String>)>::new();
    loop {
        if next_line(&mut reader, &mut line, MAX_INDEX_RECORD_BYTES)? == 0 {
            break;
        }
        let Ok(row) = serde_json::from_slice::<IndexRow>(&line) else {
            continue;
        };
        if !ids.contains(&row.id) {
            continue;
        }
        let Ok(updated) = time::OffsetDateTime::parse(
            &row.updated_at,
            &time::format_description::well_known::Rfc3339,
        ) else {
            continue;
        };
        let Ok(updated) = i64::try_from(updated.unix_timestamp_nanos() / 1_000_000) else {
            continue;
        };
        let title = label(row.thread_name.as_deref());
        match names.entry(row.id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((updated, title));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let previous = entry.get_mut();
                if updated > previous.0 {
                    *previous = (updated, title);
                } else if updated == previous.0 && title != previous.1 {
                    previous.1 = None;
                }
            }
        }
    }
    same_stamp(&open_member(&path)?, &stamp).then(|| {
        names
            .into_iter()
            .filter_map(|(id, (updated, name))| name.map(|name| (id, (updated, name))))
            .collect()
    })
}

fn label(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || !safe_display(value) {
        return None;
    }
    const LIMIT: usize = 256;
    if value.len() <= LIMIT {
        return Some(value.to_owned());
    }
    let end = value
        .grapheme_indices(true)
        .map(|(offset, grapheme)| offset + grapheme.len())
        .take_while(|end| *end <= LIMIT - '…'.len_utf8())
        .last()?;
    Some(format!("{}…", &value[..end]))
}

fn safe_display(value: &str) -> bool {
    !value.chars().any(|c| c.is_control()
        || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}

fn workspace(value: Option<&str>) -> Option<String> {
    let value = value?;
    let path = Path::new(value);
    if value.len() > 1024
        || !path.is_absolute()
        || path.components().any(|part| part == Component::ParentDir)
        || !safe_display(value)
    {
        return None;
    }
    Some(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{Observation, ObservationIdentity, ObservationKind};
    use agenturm_store::{
        ObservationBatch, SessionQuery, WorkspaceAnchor, WorkspaceGitSnapshot, WorkspaceLedger,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agenturm-native-roster-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(root.join("codex/sessions")).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn seed(
        store: &mut Store,
        path: PathBuf,
        plugin: &str,
        source: &str,
        session: &str,
        transcript: &str,
    ) -> AuthorizedSource {
        fs::write(&path, transcript).unwrap();
        let stamp = fs::metadata(&path).unwrap();
        let header = transcript.split_inclusive('\n').next().unwrap();
        let header_digest = hash(header.as_bytes());
        let dev = stamp.dev();
        let ino = stamp.ino();
        let generation = hash(format!("{dev}:{ino}:{header_digest}").as_bytes());
        let cursor = format!(
            "v2:{dev}:{ino}:{}:{}:{header_digest}:{generation}:",
            transcript.len(),
            hash(transcript.as_bytes()),
        );
        let name = ExtensionId::parse(source).unwrap();
        let batch = ObservationBatch::from_observations(
            source,
            None,
            &cursor,
            &[Observation {
                harness_id: name,
                event_id: "explicit-session".into(),
                revision: 0,
                identity: ObservationIdentity::session(session),
                observed_at_unix_ns: 1000,
                kind: ObservationKind::SessionStarted,
            }],
        )
        .unwrap();
        store.commit_batch(&batch).unwrap();
        AuthorizedSource {
            source_id: source.into(),
            plugin_id: plugin.into(),
            source_root: path,
        }
    }

    #[test]
    fn restart_recovers_retained_metadata_before_collection_and_denies_replacement() {
        let scratch = Scratch::new();
        let db = scratch.0.join("history.sqlite");
        let mut writer = Store::open_collector(&db).unwrap();
        let home = scratch.0.join("codex");
        let sources = home.join("sessions");
        let member = sources.join("retained.jsonl");
        let id = format!(
            "harness.codex.{}",
            &hash(member.to_str().unwrap().as_bytes())[..32]
        );
        let transcript = concat!(
            r#"{"type":"session_meta","payload":{"id":"exact","cwd":"/work/retained","source":"cli"}}"#,
            "\n",
        );
        seed(&mut writer, member.clone(), CODEX, &id, "exact", transcript);
        fs::write(
            home.join("session_index.jsonl"),
            concat!(
                r#"{"id":"exact","thread_name":"Retained native task","updated_at":"2026-11-23T10:00:00Z"}"#,
                "\n",
            ),
        )
        .unwrap();
        drop(writer);

        let store = Store::open_read_only(&db).unwrap();
        let page = store.query_sessions(&SessionQuery::latest(20)).unwrap();
        let follow = serde_json::from_value(serde_json::json!({
            "codex_root": sources,
            "pi_root": scratch.0.join("pi"),
            "sessions_per_harness": 1,
        }))
        .unwrap();
        let authority = SourceAuthority {
            sources: Vec::new(),
            codex_home: Some(home),
            follow: Some(follow),
        };
        let loaded = load(Some(&store), &db, &page, Some(&authority));
        let key = (ExtensionId::parse(id).unwrap(), "exact".into());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[&key].title.as_deref(), Some("Retained native task"));
        assert_eq!(loaded[&key].workspace.as_deref(), Some("/work/retained"));

        fs::rename(&member, scratch.0.join("original.jsonl")).unwrap();
        fs::write(member, transcript).unwrap();
        assert!(load(Some(&store), &db, &page, Some(&authority)).is_empty());
    }

    #[test]
    fn codex_index_renames_only_the_bound_session_and_rejects_duplicate_and_replaced_sources() {
        let scratch = Scratch::new();
        let db = scratch.0.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let home = scratch.0.join("codex");
        let member = |name: &str| home.join("sessions").join(format!("{name}.jsonl"));
        let bound = seed(
            &mut store,
            member("bound"),
            CODEX,
            "harness.codex.bound",
            "exact",
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"exact\",\"cwd\":\"/work/alpha\",\"agent_nickname\":\"Scout\",\"model_provider\":\"native\"}}\n",
        );
        let one = seed(
            &mut store,
            member("one"),
            CODEX,
            "harness.codex.one",
            "same",
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"same\",\"cwd\":\"/work/one\"}}\n",
        );
        let two = seed(
            &mut store,
            member("two"),
            CODEX,
            "harness.codex.two",
            "same",
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"same\",\"cwd\":\"/work/two\"}}\n",
        );
        let index = home.join("session_index.jsonl");
        fs::write(&index, concat!(
            "{\"id\":\"exact\",\"thread_name\":\"First title\",\"updated_at\":\"2026-11-01T00:00:00Z\"}\n",
            "{\"id\":\"same\",\"thread_name\":\"Ambiguous title\",\"updated_at\":\"2026-11-01T00:00:00Z\"}\n",
            "{\"id\":\"foreign\",\"thread_name\":\"No retained link\",\"updated_at\":\"2026-11-01T00:00:00Z\"}\n",
        )).unwrap();
        let authority = SourceAuthority {
            sources: vec![bound, one, two],
            codex_home: Some(home.clone()),
            follow: None,
        };
        let page = store.query_sessions(&SessionQuery::latest(20)).unwrap();
        let key =
            |source: &str, session: &str| (ExtensionId::parse(source).unwrap(), session.to_owned());
        let exact = key("harness.codex.bound", "exact");
        let first = load(Some(&store), &db, &page, Some(&authority));
        assert_eq!(first[&exact].title.as_deref(), Some("First title"));
        assert_eq!(first[&exact].agent_name.as_deref(), Some("Scout"));
        assert_eq!(first[&exact].model_provider.as_deref(), Some("native"));
        assert_eq!(first[&exact].workspace.as_deref(), Some("/work/alpha"));
        for (id, cwd) in [("one", "/work/one"), ("two", "/work/two")] {
            let row = &first[&key(&format!("harness.codex.{id}"), "same")];
            assert!(row.title.is_none());
            assert_eq!(row.workspace.as_deref(), Some(cwd));
        }
        assert!(first.keys().all(|key| key.1 != "foreign"));

        use std::fs::OpenOptions;
        use std::io::Write;
        OpenOptions::new().append(true).open(&index).unwrap().write_all(
            b"{\"id\":\"exact\",\"thread_name\":\"Renamed title\",\"updated_at\":\"2026-11-02T00:00:00Z\"}\n",
        ).unwrap();
        let renamed = load(Some(&store), &db, &page, Some(&authority));
        assert_eq!(renamed[&exact].title.as_deref(), Some("Renamed title"));
        assert_eq!(
            renamed.keys().collect::<Vec<_>>(),
            first.keys().collect::<Vec<_>>()
        );

        let duplicate_time = b"{\"id\":\"exact\",\"thread_name\":\"Conflict on same revision\",\"updated_at\":\"2026-11-02T00:00:00Z\"}\n";
        OpenOptions::new()
            .append(true)
            .open(&index)
            .unwrap()
            .write_all(duplicate_time)
            .unwrap();
        assert!(
            load(Some(&store), &db, &page, Some(&authority))[&exact]
                .title
                .is_none()
        );

        let replacement = scratch.0.join("other-history.sqlite");
        let mut other = Store::open_collector(&replacement).unwrap();
        other.ensure_history_identity().unwrap();
        assert!(load(Some(&store), &replacement, &page, Some(&authority)).is_empty());
        assert!(load(Some(&store), &db, &page, None).is_empty());
        assert!(load(None, &db, &page, Some(&authority)).is_empty());
        fs::rename(member("bound"), member("old")).unwrap();
        fs::copy(member("old"), member("bound")).unwrap();
        assert!(!load(Some(&store), &db, &page, Some(&authority)).contains_key(&exact));
    }

    #[test]
    fn claude_explicit_records_stay_scoped_and_saved_task_and_project_take_precedence() {
        let scratch = Scratch::new();
        let db = scratch.0.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let source = seed(
            &mut store,
            scratch.0.join("claude.jsonl"),
            CLAUDE,
            "harness.claude.main",
            "exact",
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"exact\",\"cwd\":\"/work/native\",\"message\":{\"content\":\"Prompt must never label roster\"}}\n",
                "{\"type\":\"custom-title\",\"sessionId\":\"exact\",\"customTitle\":\"First Claude name\"}\n",
                "{\"type\":\"custom-title\",\"sessionId\":\"exact\",\"customTitle\":\"Renamed Claude work\"}\n",
                "{\"type\":\"agent-name\",\"sessionId\":\"exact\",\"agentName\":\"Claude primary\"}\n",
                "{\"type\":\"agent-name\",\"sessionId\":\"other\",\"agentName\":\"Wrong session\"}\n",
                "{\"type\":\"agent-name\",\"sessionId\":\"exact\",\"agentId\":\"worker\",\"agentName\":\"Wrong worker\",\"cwd\":\"/other/worker\"}\n",
            ),
        );
        let source_path = source.source_root.clone();
        let authority = SourceAuthority {
            sources: vec![source],
            codex_home: None,
            follow: None,
        };
        let projected = super::super::super::load_session_catalog_with_sources(
            &store,
            &db,
            None,
            Some(&authority),
        )
        .unwrap();
        let row = &projected.sessions[0];
        assert_eq!(row.task_title.as_deref(), Some("Renamed Claude work"));
        assert_eq!(
            row.native_context.as_ref().unwrap().agent_name.as_deref(),
            Some("Claude primary")
        );
        assert_eq!(row.project_path.as_deref(), Some("/work/native"));
        assert_eq!(
            row.project_origin,
            Some(agenturm_ui::ProjectOrigin::NativeRecordedWorkspace)
        );

        let ledger = WorkspaceLedger::open(&db).unwrap();
        let saved = ledger
            .create_task(
                Some(&WorkspaceAnchor {
                    harness_id: ExtensionId::parse("harness.claude.main").unwrap(),
                    session_id: "exact".into(),
                    worker_id: None,
                    turn_id: None,
                }),
                "Operator title",
                "private note",
            )
            .unwrap();
        assert!(
            ledger
                .save_task_workspace_snapshot(
                    saved.id,
                    &WorkspaceGitSnapshot {
                        repository_root: "/operator/project".into(),
                        branch: None,
                        head_commit: None,
                        staged_changes: 0,
                        unstaged_changes: 0,
                        untracked_files: 0,
                        captured_at_unix_ns: 1,
                    }
                )
                .unwrap()
        );
        let current = super::super::super::load_session_catalog_with_sources(
            &store,
            &db,
            None,
            Some(&authority),
        )
        .unwrap();
        let row = &current.sessions[0];
        assert_eq!(row.task_title.as_deref(), Some("Operator title"));
        assert_eq!(row.project_path.as_deref(), Some("/operator/project"));
        assert_eq!(
            row.project_origin,
            Some(agenturm_ui::ProjectOrigin::OperatorTaskWorkspace)
        );
        let rewritten = fs::read_to_string(&source_path)
            .unwrap()
            .replace("Renamed Claude work", "Modified Claude work");
        fs::write(&source_path, rewritten).unwrap();
        let page = store.query_sessions(&SessionQuery::latest(20)).unwrap();
        assert!(load(Some(&store), &db, &page, Some(&authority)).is_empty());
    }

    #[test]
    fn claude_file_history_prelude_requires_committed_identity_and_metadata() {
        let scratch = Scratch::new();
        let db = scratch.0.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let prelude = concat!(
            r#"{"type":"file-history-snapshot","messageId":"snapshot-anchor","isSnapshotUpdate":false,"snapshot":{"messageId":"snapshot-anchor","trackedFileBackups":{"private-file":null}}}"#,
            "\n",
        );
        let user = concat!(
            r#"{"type":"user","sessionId":"exact","uuid":"different-id","cwd":"/work/verified","message":{"content":"Private prompt"}}"#,
            "\n",
        );
        let title = concat!(
            r#"{"type":"custom-title","sessionId":"exact","customTitle":"Explicit task title"}"#,
            "\n",
        );
        let source = seed(
            &mut store,
            scratch.0.join("prefixed.jsonl"),
            CLAUDE,
            "harness.claude.prefixed",
            "exact",
            &format!("{prelude}{user}{title}"),
        );
        let invalid_prelude = seed(
            &mut store,
            scratch.0.join("invalid.jsonl"),
            CLAUDE,
            "harness.claude.invalid",
            "exact",
            &format!(
                "{}{user}{title}",
                prelude.replacen(
                    r#""messageId":"snapshot-anchor""#,
                    r#""messageId":"different""#,
                    1
                ),
            ),
        );
        let path = source.source_root.clone();
        let authority = SourceAuthority {
            sources: vec![source, invalid_prelude],
            codex_home: None,
            follow: None,
        };
        let page = store.query_sessions(&SessionQuery::latest(20)).unwrap();
        let projected = load(Some(&store), &db, &page, Some(&authority));
        assert_eq!(projected.len(), 1);
        let metadata = &projected[&(
            ExtensionId::parse("harness.claude.prefixed").unwrap(),
            "exact".into(),
        )];
        assert_eq!(metadata.title.as_deref(), Some("Explicit task title"));
        assert_eq!(metadata.workspace.as_deref(), Some("/work/verified"));

        // Same physical first line, inode, and size. Changing row two's
        // workspace must still invalidate the entire context projection.
        fs::write(
            path,
            format!(
                "{prelude}{}{title}",
                user.replace("/work/verified", "/work/tampered")
            ),
        )
        .unwrap();
        assert!(load(Some(&store), &db, &page, Some(&authority)).is_empty());
    }

    #[test]
    fn oversized_claude_history_keeps_authenticated_header_and_reports_partial_title_coverage() {
        let scratch = Scratch::new();
        let db = scratch.0.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let body_line = format!(
            r#"{{"type":"assistant","sessionId":"exact","ignored":"{}"}}"#,
            "x".repeat(128 * 1024),
        );
        let mut transcript =
            String::from(r#"{"type":"user","sessionId":"exact","cwd":"/work/header-only"}"#);
        transcript.push('\n');
        while (transcript.len() as u64) <= MAX_CLAUDE_PREFIX {
            transcript.push_str(&body_line);
            transcript.push('\n');
        }
        transcript.push_str(
            r#"{"type":"custom-title","sessionId":"exact","customTitle":"Outside scan"}"#,
        );
        transcript.push('\n');
        let source = seed(
            &mut store,
            scratch.0.join("large.jsonl"),
            CLAUDE,
            "harness.claude.large",
            "exact",
            &transcript,
        );
        let authority = SourceAuthority {
            sources: vec![source],
            codex_home: None,
            follow: None,
        };
        let catalog = super::super::super::load_session_catalog_with_sources(
            &store,
            &db,
            None,
            Some(&authority),
        )
        .unwrap();
        assert_eq!(
            catalog.sessions[0].project_path.as_deref(),
            Some("/work/header-only")
        );
        assert!(catalog.sessions[0].task_title.is_none());
        assert!(
            catalog
                .notice
                .as_deref()
                .unwrap()
                .contains("Native title coverage is partial")
        );
    }

    #[test]
    fn native_labels_truncate_without_cutting_a_grapheme_and_paths_reject_control_text() {
        let long = "界e\u{301}".repeat(70);
        let title = label(Some(&long)).unwrap();
        assert!(title.len() <= 256 && title.ends_with('…'));
        assert!(long.starts_with(title.trim_end_matches('…')));
        assert!(label(Some("bad\x1b[31m")).is_none());
        assert!(workspace(Some("/../path")).is_none());
        assert!(workspace(Some("/work/unsafe\u{202e}")).is_none());
    }
}
