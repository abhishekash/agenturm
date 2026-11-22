//! Host verification for explicit Pi user-text previews.

use agenturm_kernel::ExtensionId;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "Usage: agenturm content grant --db <path> --config <path> --source-id <id>\n       agenturm content revoke --db <path> --source-id <id>\n       agenturm content list --db <path> --config <path> --source-id <id> --session-id <id>\n       agenturm content preview --db <path> --config <path> --source-id <id> [--session-id <id>] --event-id <id> --revision 0";

/// Proof-eligible metadata for a single retained user prompt. The selection
/// grants no authority and is checked anew before any text is delivered.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PiPromptReference {
    pub event_id: String,
    pub revision: u32,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiContentOptions {
    action: Action,
    db: PathBuf,
    source_id: ExtensionId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Grant {
        config: PathBuf,
    },
    Revoke,
    List {
        config: PathBuf,
        session_id: String,
    },
    Preview {
        config: PathBuf,
        session_id: Option<String>,
        event_id: String,
        revision: u32,
    },
}

pub(crate) fn parse(args: &[String]) -> Result<PiContentOptions, String> {
    let (action, flags) = args.split_first().ok_or(USAGE)?;
    let mut values = std::collections::BTreeMap::new();
    for pair in flags.chunks(2) {
        let [flag, value] = pair else {
            return Err(USAGE.into());
        };
        if value.is_empty()
            || value.starts_with("--")
            || values.insert(flag.as_str(), value.as_str()).is_some()
        {
            return Err(USAGE.into());
        }
    }
    let db = PathBuf::from(values.remove("--db").ok_or(USAGE)?);
    let source_id = ExtensionId::parse(values.remove("--source-id").ok_or(USAGE)?)
        .map_err(|_| "invalid content source ID")?;
    let action = match action.as_str() {
        "grant" => Action::Grant {
            config: PathBuf::from(values.remove("--config").ok_or(USAGE)?),
        },
        "revoke" => Action::Revoke,
        "list" => Action::List {
            config: PathBuf::from(values.remove("--config").ok_or(USAGE)?),
            session_id: values.remove("--session-id").ok_or(USAGE)?.to_owned(),
        },
        "preview" => {
            let config = PathBuf::from(values.remove("--config").ok_or(USAGE)?);
            let event_id = values.remove("--event-id").ok_or(USAGE)?;
            if event_id.len() > 256 {
                return Err("Pi preview event ID exceeds its limit".into());
            }
            let revision = values.remove("--revision").ok_or(USAGE)?;
            if revision != "0" {
                return Err("Pi preview requires native revision 0".into());
            }
            Action::Preview {
                config,
                session_id: values.remove("--session-id").map(str::to_owned),
                event_id: event_id.to_owned(),
                revision: 0,
            }
        }
        _ => return Err(USAGE.into()),
    };
    if !values.is_empty() {
        return Err(USAGE.into());
    }
    Ok(PiContentOptions {
        action,
        db,
        source_id,
    })
}

pub fn list_pi_user_prompts(
    db: &Path,
    config: &Path,
    source_id: &ExtensionId,
    session_id: &str,
) -> Result<Vec<PiPromptReference>, String> {
    #[cfg(target_os = "linux")]
    {
        linux::list(&linux::open_store(db)?, config, source_id, session_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (db, config, source_id, session_id);
        Err("Pi content preview is available on Linux".into())
    }
}

/// Sends an exact user-selected prompt to its final sink while the proof
/// and authorization fence is held. The sink must complete delivery on flush.
pub fn reveal_pi_user_prompt(
    db: &Path,
    config: &Path,
    source_id: &ExtensionId,
    session_id: &str,
    event_id: &str,
    revision: u32,
    output: &mut dyn Write,
) -> Result<(), String> {
    execute_content(
        &PiContentOptions {
            action: Action::Preview {
                config: config.to_owned(),
                session_id: Some(session_id.to_owned()),
                event_id: event_id.to_owned(),
                revision,
            },
            db: db.to_owned(),
            source_id: source_id.clone(),
        },
        output,
    )
}

pub fn execute_content(options: &PiContentOptions, output: &mut dyn Write) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::execute(options, output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (options, output);
        Err("Pi content preview is available on Linux".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{discovery::PI_PLUGIN_ID, load_collection_config};
    use agenturm_jsonl_source::{CursorProof, cursor_proof};
    use agenturm_store::{ContentGrantScope, ContentKind, PiUserTextEvidence, Store};
    use rustix::fs::{Mode, OFlags, open, openat};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs::{File, Metadata};
    use std::io::Read;
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    use std::path::Component;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    const KIND: ContentKind = ContentKind::PiUserText;
    const MAX_PREFIX: u64 = 8 * 1024 * 1024;
    const MAX_ROW: usize = 8 * 1024;
    const MAX_TEXT_BYTES: usize = 2 * 1024;
    const SOURCE_CHANGED: &str = "Pi source changed or no longer matches the committed checkpoint";
    const NO_EVIDENCE: &str = "selected revision is unavailable in the current Pi source commit";

    pub(super) fn execute(
        options: &PiContentOptions,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        let mut store = open_store(&options.db)?;
        match &options.action {
            Action::Grant { config } => {
                let source = BoundSource::open(config, &options.source_id)?;
                store
                    .grant_content(&source.scope)
                    .map_err(|error| error.to_string())?;
                write_output(output, b"Approved Pi user-text preview for this source.\n")
            }
            Action::Revoke => {
                store
                    .revoke_content(&options.source_id, KIND)
                    .map_err(|error| error.to_string())?;
                write_output(output, b"Pi user-text preview approval is revoked.\n")
            }
            Action::List { config, session_id } => {
                let references = list(&store, config, &options.source_id, session_id)?;
                let mut line = serde_json::to_vec(&references)
                    .map_err(|_| "cannot encode Pi prompt references")?;
                line.push(b'\n');
                write_output(output, &line)
            }
            Action::Preview {
                config,
                session_id,
                event_id,
                revision,
            } => {
                if *revision != 0 {
                    return Err("Pi preview requires native revision 0".into());
                }
                store
                    .with_pi_content_fence(|store| {
                        preview(
                            store,
                            config,
                            &options.source_id,
                            session_id.as_deref(),
                            event_id,
                            output,
                        )
                    })
                    .map_err(|error| error.to_string())?
            }
        }
    }

    pub(super) fn open_store(db: &Path) -> Result<Store, String> {
        if !db.is_file() {
            return Err("Pi content commands require an existing observation database".into());
        }
        Store::open_collector(db).map_err(|error| error.to_string())
    }

    pub(super) fn list(
        store: &Store,
        config: &Path,
        source_id: &ExtensionId,
        session_id: &str,
    ) -> Result<Vec<PiPromptReference>, String> {
        if !valid_id(session_id) {
            return Err("invalid selected Pi session ID".into());
        }
        store
            .with_pi_content_fence(|store| {
                let mut source = BoundSource::open(config, source_id)?;
                let grant = store
                    .current_content_grant(&source.scope)
                    .map_err(|error| error.to_string())?
                    .ok_or("Pi user-text preview requires approval for the current source")?;
                let evidence = store
                    .current_pi_user_text_references(&source.scope, &grant, session_id)
                    .map_err(|error| error.to_string())?;
                let Some((_, first)) = evidence.first() else {
                    return Ok(Vec::new());
                };
                let prefix = source.verified_prefix(&first.cursor)?;
                let rows = prefix.rows(
                    evidence
                        .iter()
                        .filter_map(|(event, evidence)| prefix.row_id(evidence, event).ok()),
                )?;
                let mut references = Vec::new();
                for (event_id, evidence) in &evidence {
                    if evidence.cursor != first.cursor {
                        return Err(SOURCE_CHANGED.into());
                    }
                    // An otherwise valid reference can be ineligible for
                    // disclosure, for example if its text looks like a credential.
                    if prefix.text(evidence, event_id, &rows).is_ok() {
                        references.push(PiPromptReference {
                            event_id: event_id.clone(),
                            revision: 0,
                            turn_id: evidence.turn_id.clone(),
                            observed_at_unix_ns: evidence.observed_at_unix_ns,
                        });
                    }
                }
                let fresh = source.recheck(config, source_id)?;
                store
                    .verify_content_grant(&fresh.scope, &grant)
                    .map_err(|error| error.to_string())?;
                Ok(references)
            })
            .map_err(|error| error.to_string())?
    }

    fn preview(
        store: &Store,
        config: &Path,
        source_id: &ExtensionId,
        session_id: Option<&str>,
        event_id: &str,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        if event_id.is_empty() || event_id.len() > 256 {
            return Err("invalid Pi preview event ID".into());
        }
        if session_id.is_some_and(|id| !valid_id(id)) {
            return Err("invalid selected Pi session ID".into());
        }
        let mut source = BoundSource::open(config, source_id)?;
        let grant = store
            .current_content_grant(&source.scope)
            .map_err(|error| error.to_string())?
            .ok_or("Pi user-text preview requires approval for the current source")?;
        let evidence = store
            .current_pi_user_text_evidence(&source.scope, &grant, event_id, 0)
            .map_err(|error| error.to_string())?
            .filter(|evidence| session_id.is_none_or(|session| evidence.session_id == session))
            .ok_or(NO_EVIDENCE)?;
        let prefix = source.verified_prefix(&evidence.cursor)?;
        let id = prefix.row_id(&evidence, event_id)?;
        let rows = prefix.rows(std::iter::once(id))?;
        let text = prefix.text(&evidence, event_id, &rows)?;
        let fresh = source.recheck(config, source_id)?;
        if store
            .current_pi_user_text_evidence(&fresh.scope, &grant, event_id, 0)
            .map_err(|error| error.to_string())?
            .as_ref()
            != Some(&evidence)
        {
            return Err(NO_EVIDENCE.into());
        }
        if FileStamp::from_file(&fresh.file)? != fresh.stamp {
            return Err(SOURCE_CHANGED.into());
        }
        // JSON escaping keeps controls out of terminal output. The source
        // validator has already removed C1 and Unicode direction controls.
        let mut line =
            serde_json::to_vec(&text).map_err(|_| "cannot encode Pi user-text preview")?;
        line.push(b'\n');
        write_output(output, &line)
    }

    fn write_output(output: &mut dyn Write, bytes: &[u8]) -> Result<(), String> {
        output
            .write_all(bytes)
            .and_then(|()| output.flush())
            .map_err(|_| "cannot write Pi content output".into())
    }

    #[derive(Eq, PartialEq)]
    struct FileStamp {
        device: u64,
        inode: u64,
        bytes: u64,
        modified: (i64, i64),
        changed: (i64, i64),
    }

    impl FileStamp {
        fn from_file(file: &File) -> Result<Self, String> {
            let metadata = file.metadata().map_err(|_| SOURCE_CHANGED)?;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err("Pi content requires a singly linked regular JSONL file".into());
            }
            Ok(Self::new(&metadata))
        }

        fn new(metadata: &Metadata) -> Self {
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                bytes: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            }
        }
    }

    struct BoundSource {
        file: File,
        stamp: FileStamp,
        scope: ContentGrantScope,
    }

    impl BoundSource {
        fn open(config: &Path, source_id: &ExtensionId) -> Result<Self, String> {
            let config = load_collection_config(config)?;
            let source = config
                .sources
                .iter()
                .find(|source| source.source_id == source_id.as_str())
                .ok_or("Pi content source is absent from the collection configuration")?;
            let artifact = source
                .artifact_sha256
                .as_deref()
                .filter(|_| source.plugin_id == PI_PLUGIN_ID)
                .ok_or(
                    "Pi content requires a SHA-256-pinned Pi adapter in catalog configuration",
                )?;
            let file = open_member(&source.source_root)?;
            let stamp = FileStamp::from_file(&file)?;
            let feature_set = source.features.as_deref().unwrap_or_default();
            let mut features = feature_set.iter().map(String::as_bytes).collect::<Vec<_>>();
            features.sort_unstable();
            let device = stamp.device.to_be_bytes();
            let inode = stamp.inode.to_be_bytes();
            let mut fingerprint = Sha256::new();
            for field in [
                b"agenturm.pi-user-text-preview/1".as_slice(),
                source.source_id.as_bytes(),
                source.plugin_id.as_bytes(),
                artifact.as_bytes(),
                config.plugin_host.as_os_str().as_bytes(),
                source.adapter.as_os_str().as_bytes(),
                source.source_root.as_os_str().as_bytes(),
                &device,
                &inode,
            ]
            .into_iter()
            .chain(features)
            {
                fingerprint.update((field.len() as u64).to_be_bytes());
                fingerprint.update(field);
            }
            let scope =
                ContentGrantScope::new(source_id.clone(), KIND, fingerprint.finalize().into());
            Ok(Self { file, stamp, scope })
        }

        fn verified_prefix(&mut self, committed_cursor: &str) -> Result<VerifiedPrefix, String> {
            let cursor = cursor_proof(committed_cursor)
                .map_err(|_| SOURCE_CHANGED)?
                .filter(|cursor| {
                    cursor.version == 2
                        && cursor.device == self.stamp.device
                        && cursor.inode == self.stamp.inode
                        && (1..=MAX_PREFIX).contains(&cursor.offset)
                        && cursor.offset <= self.stamp.bytes
                })
                .ok_or(SOURCE_CHANGED)?;
            let canonical = hash(
                format!(
                    "{}:{}:{}",
                    cursor.device, cursor.inode, cursor.header_digest
                )
                .as_bytes(),
            );
            if cursor.generation != canonical {
                return Err("Pi preview requires the source's initial verified generation".into());
            }
            let mut bytes = Vec::new();
            (&mut self.file)
                .take(cursor.offset)
                .read_to_end(&mut bytes)
                .map_err(|_| SOURCE_CHANGED)?;
            if bytes.len() as u64 != cursor.offset
                || bytes.last() != Some(&b'\n')
                || hash(&bytes) != cursor.digest
                || FileStamp::from_file(&self.file)? != self.stamp
            {
                return Err(SOURCE_CHANGED.into());
            }
            let header = bytes
                .split_inclusive(|byte| *byte == b'\n')
                .next()
                .ok_or(SOURCE_CHANGED)?;
            if header.len() > MAX_ROW || hash(header) != cursor.header_digest {
                return Err(SOURCE_CHANGED.into());
            }
            let header_len = header.len();
            let header = serde_json::from_slice::<Header>(header).map_err(|_| SOURCE_CHANGED)?;
            if header.kind != "session" || !valid_id(&header.id) {
                return Err(SOURCE_CHANGED.into());
            }
            Ok(VerifiedPrefix {
                bytes,
                cursor,
                header,
                header_len,
            })
        }

        fn recheck(&self, config: &Path, source_id: &ExtensionId) -> Result<Self, String> {
            let fresh = Self::open(config, source_id)?;
            if self.stamp != fresh.stamp
                || self.scope != fresh.scope
                || FileStamp::from_file(&fresh.file)? != fresh.stamp
            {
                return Err(SOURCE_CHANGED.into());
            }
            Ok(fresh)
        }
    }

    struct VerifiedPrefix {
        bytes: Vec<u8>,
        cursor: CursorProof,
        header: Header,
        header_len: usize,
    }

    enum RowMatch<'a> {
        Missing,
        Exact(&'a [u8]),
        Ineligible,
    }

    impl VerifiedPrefix {
        fn row_id<'a>(
            &self,
            evidence: &PiUserTextEvidence,
            event_id: &'a str,
        ) -> Result<&'a str, String> {
            let worker_id = self.header.agent_id.as_deref().filter(|id| !id.is_empty());
            if self.header.session_id.as_deref().unwrap_or(&self.header.id) != evidence.session_id
                || worker_id != evidence.worker_id.as_deref()
            {
                return Err(SOURCE_CHANGED.into());
            }
            event_id
                .strip_prefix("pi:")
                .and_then(|id| id.strip_prefix(&self.cursor.generation))
                .and_then(|id| id.strip_prefix(':'))
                .and_then(|id| id.strip_suffix(":message"))
                .filter(|id| *id == evidence.turn_id && valid_id(id))
                .ok_or_else(|| NO_EVIDENCE.into())
        }

        fn rows<'a, 'b>(
            &'a self,
            selected: impl Iterator<Item = &'b str>,
        ) -> Result<BTreeMap<&'b str, RowMatch<'a>>, String> {
            let mut matches = selected
                .map(|id| (id, RowMatch::Missing))
                .collect::<BTreeMap<_, _>>();
            for line in self.bytes[self.header_len..].split_inclusive(|byte| *byte == b'\n') {
                let identity = serde_json::from_slice::<RowId>(line).map_err(|_| SOURCE_CHANGED)?;
                if let Some(slot) = identity.id.as_deref().and_then(|id| matches.get_mut(id)) {
                    *slot = if matches!(slot, RowMatch::Missing) && line.len() <= MAX_ROW {
                        RowMatch::Exact(line)
                    } else {
                        RowMatch::Ineligible
                    };
                }
            }
            Ok(matches)
        }

        fn text(
            &self,
            evidence: &PiUserTextEvidence,
            event_id: &str,
            rows: &BTreeMap<&str, RowMatch<'_>>,
        ) -> Result<String, String> {
            let id = self.row_id(evidence, event_id)?;
            let line = match rows.get(id) {
                Some(RowMatch::Exact(line)) => *line,
                Some(RowMatch::Ineligible) => {
                    return Err("Pi preview row is ambiguous or exceeds its proof limit".into());
                }
                _ => return Err(NO_EVIDENCE.into()),
            };
            let row = serde_json::from_slice::<UserRow>(line).map_err(|_| NO_EVIDENCE)?;
            let timestamp = OffsetDateTime::parse(&row.timestamp, &Rfc3339)
                .ok()
                .and_then(|time| u64::try_from(time.unix_timestamp_nanos()).ok());
            if row.kind != "message"
                || row.message.role != "user"
                || timestamp != Some(evidence.observed_at_unix_ns)
            {
                return Err(NO_EVIDENCE.into());
            }
            text_preview(row.message.content)
        }
    }

    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        #[serde(rename = "agentId")]
        agent_id: Option<String>,
    }

    #[derive(Deserialize)]
    struct RowId {
        id: Option<String>,
    }

    #[derive(Deserialize)]
    struct UserRow {
        #[serde(rename = "type")]
        kind: String,
        timestamp: String,
        message: UserMessage,
    }

    #[derive(Deserialize)]
    struct UserMessage {
        role: String,
        content: UserContent,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum UserContent {
        Text(String),
        Blocks(Vec<TextBlock>),
    }

    #[derive(Deserialize)]
    struct TextBlock {
        #[serde(rename = "type")]
        kind: String,
        text: Option<String>,
    }

    fn text_preview(content: UserContent) -> Result<String, String> {
        let text = match content {
            UserContent::Text(text) => text,
            UserContent::Blocks(blocks) => blocks
                .into_iter()
                .filter(|block| matches!(block.kind.as_str(), "text" | "input_text"))
                .filter_map(|block| block.text)
                .collect::<Vec<_>>()
                .join("\n"),
        };
        if text.is_empty() || text.len() > MAX_TEXT_BYTES {
            return Err("Pi content is unavailable within the preview text limit".into());
        }
        let lower = text.to_ascii_lowercase();
        let known_credential = [
            "private key",
            "sk-",
            "sk_live_",
            "sk_test_",
            "rk_live_",
            "rk_test_",
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "ghr_",
            "github_pat_",
            "akia",
            "aiza",
            "xoxb-",
            "xoxa-",
            "xoxp-",
            "xoxr-",
            "xoxs-",
        ]
        .iter()
        .any(|marker| lower.contains(marker));
        let password_command = lower.lines().any(|line| {
            let mut words = line.split_ascii_whitespace();
            words.next() == Some("mysql")
                && words.any(|word| word.starts_with("-p") && word.len() > 2)
        });
        let bearer_value = lower.split_ascii_whitespace().any(|word| word == "bearer");
        let assigned_credential = lower.lines().any(|line| {
            (line.contains('=') || line.contains(':'))
                && [
                    "authorization",
                    "password",
                    "passwd",
                    "token",
                    "secret",
                    "credential",
                    "api_key",
                    "api-key",
                    "apikey",
                ]
                .iter()
                .any(|marker| line.contains(marker))
        });
        let uri_credential = lower.split_ascii_whitespace().any(|word| {
            word.split_once("://")
                .and_then(|(_, tail)| tail.split('/').next())
                .is_some_and(|authority| authority.contains(':') && authority.contains('@'))
        });
        if known_credential
            || password_command
            || bearer_value
            || assigned_credential
            || uri_credential
        {
            return Err(
                "Pi preview withheld because it contains possible credential material".into(),
            );
        }
        let text: String = text
            .chars()
            .filter(|&character| {
                matches!(character, '\n' | '\t')
                    || (!character.is_control()
                        && !matches!(
                            character,
                            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                        ))
            })
            .collect();
        if text.is_empty() {
            return Err("Pi content is unavailable within the preview text limit".into());
        }
        Ok(text)
    }

    fn open_member(path: &Path) -> Result<File, String> {
        let unavailable = "cannot open the configured Pi member without following links";
        let mut components = path.components();
        if components.next() != Some(Component::RootDir)
            || path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
        {
            return Err("Pi content requires one configured JSONL file".into());
        }
        let directory_flags =
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory = open("/", directory_flags, Mode::empty()).map_err(|_| unavailable)?;
        let mut components = components.peekable();
        while let Some(Component::Normal(name)) = components.next() {
            if components.peek().is_none() {
                let member_flags =
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
                return openat(directory, name, member_flags, Mode::empty())
                    .map(File::from)
                    .map_err(|_| unavailable.into());
            }
            directory =
                openat(directory, name, directory_flags, Mode::empty()).map_err(|_| unavailable)?;
        }
        Err(unavailable.into())
    }

    fn valid_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 160
            && id.as_bytes()[0].is_ascii_alphanumeric()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    }

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
}
