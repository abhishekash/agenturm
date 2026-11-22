//! Host verification for explicit user-text previews from installed harnesses.

use agenturm_kernel::ExtensionId;
use agenturm_store::Store;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "Usage: agenturm content grant --db <path> --config <path> --source-id <id>\n       agenturm content revoke --db <path> --source-id <id>\n       agenturm content list --db <path> --config <path> --source-id <id> --session-id <id>\n       agenturm content preview --db <path> --config <path> --source-id <id> [--session-id <id>] --event-id <id> --revision <revision>";

/// Proof-eligible metadata for a single retained user prompt. The selection
/// grants no authority and is checked anew before any text is delivered.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PromptReference {
    pub event_id: String,
    pub revision: u32,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentOptions {
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

pub(crate) fn parse(args: &[String]) -> Result<ContentOptions, String> {
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
                return Err("Prompt preview event ID exceeds its limit".into());
            }
            let revision = values
                .remove("--revision")
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or("invalid prompt revision")?;
            Action::Preview {
                config,
                session_id: values.remove("--session-id").map(str::to_owned),
                event_id: event_id.to_owned(),
                revision,
            }
        }
        _ => return Err(USAGE.into()),
    };
    if !values.is_empty() {
        return Err(USAGE.into());
    }
    Ok(ContentOptions {
        action,
        db,
        source_id,
    })
}

pub fn list_user_prompts(
    db: &Path,
    config: &Path,
    source_id: &ExtensionId,
    session_id: &str,
    writer: Option<&Store>,
) -> Result<Vec<PromptReference>, String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(writer) = writer {
            linux::list(writer, config, source_id, session_id)
        } else {
            linux::list(&linux::open_store(db)?, config, source_id, session_id)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (db, config, source_id, session_id, writer);
        Err("Prompt preview is available on Linux".into())
    }
}

/// Sends an exact user-selected prompt to its final sink while the proof
/// and authorization fence is held. The sink must complete delivery on flush.
pub(super) struct PromptSelection<'a> {
    pub session_id: &'a str,
    pub event_id: &'a str,
    pub revision: u32,
}

pub(super) fn reveal_user_prompt(
    db: &Path,
    config: &Path,
    source_id: &ExtensionId,
    selection: PromptSelection<'_>,
    output: &mut dyn Write,
    writer: Option<&Store>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if let Some(writer) = writer {
        return linux::reveal_fenced(
            writer,
            config,
            source_id,
            Some(selection.session_id),
            selection.event_id,
            selection.revision,
            output,
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = writer;
    execute_content(
        &ContentOptions {
            action: Action::Preview {
                config: config.to_owned(),
                session_id: Some(selection.session_id.to_owned()),
                event_id: selection.event_id.to_owned(),
                revision: selection.revision,
            },
            db: db.to_owned(),
            source_id: source_id.clone(),
        },
        output,
    )
}

pub fn execute_content(options: &ContentOptions, output: &mut dyn Write) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::execute(options, output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (options, output);
        Err("Prompt preview is available on Linux".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{
        discovery::{CLAUDE_PLUGIN_ID, KIRO_PLUGIN_ID, OPENCODE_PLUGIN_ID, PI_PLUGIN_ID},
        load_collection_config,
    };
    use agenturm_jsonl_source::{CursorProof, cursor_proof};
    use agenturm_store::{ContentGrantScope, ContentKind, UserTextEvidence};
    use rustix::fs::{Mode, OFlags, open, openat};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs::{File, Metadata};
    use std::io::Read;
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    use std::path::Component;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Harness {
        Pi,
        Codex,
        Claude,
        Kiro,
        OpenCode,
    }

    impl Harness {
        fn content_kind(self) -> ContentKind {
            match self {
                Self::Pi => ContentKind::PiUserText,
                Self::Codex => ContentKind::CodexUserText,
                Self::Claude => ContentKind::ClaudeUserText,
                Self::Kiro => ContentKind::KiroUserText,
                Self::OpenCode => ContentKind::OpenCodeUserText,
            }
        }

        fn label(self) -> &'static str {
            match self {
                Self::Pi => "Pi",
                Self::Codex => "Codex",
                Self::Claude => "Claude Code",
                Self::Kiro => "Kiro",
                Self::OpenCode => "OpenCode",
            }
        }

        fn fingerprint_domain(self) -> &'static [u8] {
            match self {
                Self::Pi => b"agenturm.pi-user-text-preview/1",
                Self::Codex => b"agenturm.codex-user-text-preview/1",
                Self::Claude => b"agenturm.claude-user-text-preview/1",
                Self::Kiro => b"agenturm.kiro-user-text-preview/1",
                Self::OpenCode => b"agenturm.opencode-user-text-preview/1",
            }
        }
    }
    const MAX_PREFIX: u64 = 8 * 1024 * 1024;
    const MAX_ROW: usize = 8 * 1024;
    const MAX_TEXT_BYTES: usize = 2 * 1024;
    const SOURCE_CHANGED: &str =
        "Prompt source changed or no longer matches the committed checkpoint";
    const NO_EVIDENCE: &str = "selected revision is unavailable in the current source commit";

    pub(super) fn execute(options: &ContentOptions, output: &mut dyn Write) -> Result<(), String> {
        let mut store = open_store(&options.db)?;
        match &options.action {
            Action::Grant { config } => {
                let source = BoundSource::open(config, &options.source_id)?;
                store
                    .grant_content(&source.scope)
                    .map_err(|error| error.to_string())?;
                write_output(
                    output,
                    format!(
                        "Approved {} user-text preview for this source.\n",
                        source.harness.label()
                    )
                    .as_bytes(),
                )
            }
            Action::Revoke => {
                store
                    .revoke_source_content(&options.source_id)
                    .map_err(|error| error.to_string())?;
                write_output(output, b"User-text preview approval is revoked.\n")
            }
            Action::List { config, session_id } => {
                let references = list(&store, config, &options.source_id, session_id)?;
                let mut line = serde_json::to_vec(&references)
                    .map_err(|_| "cannot encode prompt references")?;
                line.push(b'\n');
                write_output(output, &line)
            }
            Action::Preview {
                config,
                session_id,
                event_id,
                revision,
            } => reveal_fenced(
                &store,
                config,
                &options.source_id,
                session_id.as_deref(),
                event_id,
                *revision,
                output,
            ),
        }
    }

    pub(super) fn open_store(db: &Path) -> Result<Store, String> {
        if !db.is_file() {
            return Err("Content commands require an existing observation database".into());
        }
        Store::open_collector(db).map_err(|error| error.to_string())
    }

    pub(super) fn list(
        store: &Store,
        config: &Path,
        source_id: &ExtensionId,
        session_id: &str,
    ) -> Result<Vec<PromptReference>, String> {
        if !valid_id(session_id) {
            return Err("invalid selected session ID".into());
        }
        store
            .with_user_content_fence(|store| {
                let mut source = BoundSource::open(config, source_id)?;
                let grant = store
                    .current_content_grant(&source.scope)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| missing_approval(source.harness))?;
                let evidence = store
                    .current_user_text_references(&source.scope, &grant, session_id)
                    .map_err(|error| error.to_string())?;
                let Some((_, first)) = evidence.first() else {
                    return Ok(Vec::new());
                };
                let texts = source.prove(&evidence, false)?;

                let mut references = Vec::new();
                for (event_id, evidence) in &evidence {
                    if evidence.cursor != first.cursor {
                        return Err(SOURCE_CHANGED.into());
                    }
                    // An otherwise valid reference can be ineligible for
                    // disclosure, for example if its text looks like a credential.
                    if texts.contains_key(event_id) {
                        references.push(PromptReference {
                            event_id: event_id.clone(),
                            revision: evidence.revision,
                            turn_id: evidence.turn_id.clone(),
                            observed_at_unix_ns: evidence.observed_at_unix_ns,
                        });
                    }
                }
                let fresh = source.recheck(config, source_id)?;
                store
                    .verify_content_grant(&fresh.scope, &grant)
                    .map_err(|error| error.to_string())?;
                fresh.verify_additional_checkpoint(&first.cursor)?;
                Ok(references)
            })
            .map_err(|error| error.to_string())?
    }

    pub(super) fn reveal_fenced(
        writer: &Store,
        config: &Path,
        source_id: &ExtensionId,
        session_id: Option<&str>,
        event_id: &str,
        revision: u32,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        writer
            .with_user_content_fence(|store| {
                preview(
                    store, config, source_id, session_id, event_id, revision, output,
                )
            })
            .map_err(|error| error.to_string())?
    }

    fn preview(
        store: &Store,
        config: &Path,
        source_id: &ExtensionId,
        session_id: Option<&str>,
        event_id: &str,
        revision: u32,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        if event_id.is_empty() || event_id.len() > 256 {
            return Err("invalid prompt preview event ID".into());
        }
        if session_id.is_some_and(|id| !valid_id(id)) {
            return Err("invalid selected session ID".into());
        }
        let mut source = BoundSource::open(config, source_id)?;
        let grant = store
            .current_content_grant(&source.scope)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| missing_approval(source.harness))?;
        let evidence = store
            .current_user_text_evidence(&source.scope, &grant, event_id, revision)
            .map_err(|error| error.to_string())?
            .filter(|evidence| session_id.is_none_or(|session| evidence.session_id == session))
            .ok_or(NO_EVIDENCE)?;
        let wanted = [(event_id.to_owned(), evidence.clone())];
        let text = source
            .prove(&wanted, true)?
            .remove(event_id)
            .ok_or(NO_EVIDENCE)?;
        let fresh = source.recheck(config, source_id)?;
        if store
            .current_user_text_evidence(&fresh.scope, &grant, event_id, revision)
            .map_err(|error| error.to_string())?
            .as_ref()
            != Some(&evidence)
        {
            return Err(NO_EVIDENCE.into());
        }
        if FileStamp::from_file(&fresh.file)? != fresh.stamp {
            return Err(SOURCE_CHANGED.into());
        }
        fresh.verify_additional_checkpoint(&evidence.cursor)?;
        // JSON escaping keeps controls out of terminal output. The source
        // validator has already removed C1 and Unicode direction controls.
        let mut line = serde_json::to_vec(&text).map_err(|_| "cannot encode user-text preview")?;
        line.push(b'\n');
        write_output(output, &line)
    }

    fn missing_approval(harness: Harness) -> String {
        format!(
            "{} user-text preview requires approval for the current source",
            harness.label()
        )
    }

    fn write_output(output: &mut dyn Write, bytes: &[u8]) -> Result<(), String> {
        output
            .write_all(bytes)
            .and_then(|()| output.flush())
            .map_err(|_| "cannot write content output".into())
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
                return Err("Content preview requires a singly linked regular file".into());
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
        harness: Harness,
        source_path: PathBuf,
    }

    impl BoundSource {
        fn open(config: &Path, source_id: &ExtensionId) -> Result<Self, String> {
            let config = load_collection_config(config)?;
            let source = config
                .sources
                .iter()
                .find(|source| source.source_id == source_id.as_str())
                .ok_or("Content source is absent from the collection configuration")?;
            let harness = match source.plugin_id.as_str() {
                PI_PLUGIN_ID => Harness::Pi,
                agenturm_runtime::CODEX_PLUGIN_ID => Harness::Codex,
                CLAUDE_PLUGIN_ID => Harness::Claude,
                KIRO_PLUGIN_ID => Harness::Kiro,
                OPENCODE_PLUGIN_ID => Harness::OpenCode,
                _ => {
                    return Err("Prompt preview requires a supported pinned source".into());
                }
            };
            let artifact = source.artifact_sha256.as_deref().ok_or(
                "Prompt preview requires a SHA-256-pinned adapter in catalog configuration",
            )?;
            let file = open_member(&source.source_root, harness)?;
            let stamp = FileStamp::from_file(&file)?;
            let feature_set = source.features.as_deref().unwrap_or_default();
            let mut features = feature_set.iter().map(String::as_bytes).collect::<Vec<_>>();
            features.sort_unstable();
            let device = stamp.device.to_be_bytes();
            let inode = stamp.inode.to_be_bytes();
            let mut fingerprint = Sha256::new();
            for field in [
                harness.fingerprint_domain(),
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
            let scope = ContentGrantScope::new(
                source_id.clone(),
                harness.content_kind(),
                fingerprint.finalize().into(),
            );
            Ok(Self {
                file,
                stamp,
                scope,
                harness,
                source_path: source.source_root.clone(),
            })
        }

        fn prove(
            &mut self,
            evidence: &[(String, UserTextEvidence)],
            exact: bool,
        ) -> Result<BTreeMap<String, String>, String> {
            let Some((_, first)) = evidence.first() else {
                return Ok(BTreeMap::new());
            };
            if evidence
                .iter()
                .any(|(_, row)| row.cursor != first.cursor || row.session_id != first.session_id)
            {
                return Err(SOURCE_CHANGED.into());
            }
            if self.harness == Harness::OpenCode {
                let candidates = evidence
                    .iter()
                    .filter(|(_, row)| row.worker_id.is_none())
                    .map(
                        |(event, row)| agenturm_opencode_harness::UserPromptEvidence {
                            event_id: event,
                            turn_id: &row.turn_id,
                            revision: row.revision,
                            observed_at_unix_ns: row.observed_at_unix_ns,
                        },
                    )
                    .collect::<Vec<_>>();
                let texts = agenturm_opencode_harness::prove_user_prompts(
                    &self.source_path,
                    &first.cursor,
                    &first.session_id,
                    &candidates,
                )
                .map_err(|error| error.to_string())?;
                let texts = texts
                    .into_iter()
                    .filter_map(|(event, text)| {
                        text_preview(UserContent::Text(text))
                            .ok()
                            .map(|text| (event, text))
                    })
                    .collect();
                return Ok(texts);
            }
            let prefix = self.verified_prefix(&first.cursor)?;
            let admitted = prefix.admitted_offsets(
                evidence
                    .iter()
                    .filter_map(|(_, row)| row.admitted_cursor.as_deref()),
            );
            let rows = prefix.rows(
                evidence
                    .iter()
                    .filter_map(|(event, row)| prefix.row_id(row, event).ok()),
            )?;
            let mut texts = BTreeMap::new();
            for (event, row) in evidence {
                let proof_end = match row.admitted_cursor.as_deref() {
                    Some(proof) => admitted.get(proof).copied(),
                    None => Some(prefix.bytes.len()),
                };
                let verified = proof_end
                    .ok_or_else(|| NO_EVIDENCE.to_owned())
                    .and_then(|end| prefix.text(row, event, &rows, end));
                match verified {
                    Ok(text) => {
                        texts.insert(event.clone(), text);
                    }
                    Err(error) if exact => return Err(error),
                    Err(_) => {}
                }
            }
            Ok(texts)
        }

        fn verify_additional_checkpoint(&self, cursor: &str) -> Result<(), String> {
            if self.harness == Harness::OpenCode {
                agenturm_opencode_harness::verify_prompt_checkpoint(&self.source_path, cursor)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
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
                return Err(
                    "Prompt preview requires the source's initial verified generation".into(),
                );
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
            let header = match self.harness {
                Harness::Pi => {
                    let header: PiHeader =
                        serde_json::from_slice(header).map_err(|_| SOURCE_CHANGED)?;
                    if header.kind != "session" || !valid_id(&header.id) {
                        return Err(SOURCE_CHANGED.into());
                    }
                    Header::Pi(header)
                }
                Harness::Codex => {
                    let header: CodexHeader =
                        serde_json::from_slice(header).map_err(|_| SOURCE_CHANGED)?;
                    if header.kind != "session_meta"
                        || !valid_id(&header.payload.id)
                        || header
                            .payload
                            .session_id
                            .as_ref()
                            .is_some_and(|root| root != &header.payload.id)
                        || header
                            .payload
                            .source
                            .as_ref()
                            .is_some_and(|source| source.get("subagent").is_some())
                    {
                        return Err(SOURCE_CHANGED.into());
                    }
                    Header::Codex(header.payload.id)
                }
                Harness::Claude => {
                    let header: ClaudeHeader =
                        serde_json::from_slice(header).map_err(|_| SOURCE_CHANGED)?;
                    if !valid_id(&header.session_id) {
                        return Err(SOURCE_CHANGED.into());
                    }
                    Header::Claude(header.session_id)
                }
                Harness::OpenCode => return Err(SOURCE_CHANGED.into()),
                Harness::Kiro => {
                    let session = self
                        .source_path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .filter(|name| name.starts_with("sess_") && valid_id(name))
                        .filter(|_| {
                            self.source_path
                                .file_name()
                                .is_some_and(|name| name == "messages.jsonl")
                        })
                        .ok_or("Kiro prompt preview requires the main session transcript")?;
                    Header::Kiro(session.to_owned())
                }
            };
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
        Exact { line: &'a [u8], end: usize },
        Ineligible,
    }

    impl VerifiedPrefix {
        fn admitted_offsets<'a>(
            &self,
            receipts: impl Iterator<Item = &'a str>,
        ) -> BTreeMap<&'a str, usize> {
            let mut receipts: Vec<_> = receipts
                .filter_map(|raw| {
                    let proof = cursor_proof(raw).ok().flatten()?;
                    let end = usize::try_from(proof.offset).ok()?;
                    (proof.version == 2
                        && proof.device == self.cursor.device
                        && proof.inode == self.cursor.inode
                        && proof.header_digest == self.cursor.header_digest
                        && proof.generation == self.cursor.generation
                        && end >= self.header_len
                        && end <= self.bytes.len()
                        && self.bytes.get(end - 1) == Some(&b'\n'))
                    .then_some((end, raw, proof.digest))
                })
                .collect();
            receipts.sort_unstable_by_key(|(end, _, _)| *end);
            let mut outputs = BTreeMap::new();
            let mut digester = Sha256::new();
            let mut position = 0;
            let mut present = String::new();
            for (end, key, expected) in receipts {
                if end > position {
                    digester.update(&self.bytes[position..end]);
                    present = format!("{:x}", digester.clone().finalize());
                    position = end;
                }
                if expected == present {
                    outputs.insert(key, end);
                }
            }
            outputs
        }

        fn row_id(&self, evidence: &UserTextEvidence, event_id: &str) -> Result<String, String> {
            let (prefix, suffix) = match &self.header {
                Header::Pi(header) => {
                    let worker_id = header.agent_id.as_deref().filter(|id| !id.is_empty());
                    if header.session_id.as_deref().unwrap_or(&header.id) != evidence.session_id
                        || worker_id != evidence.worker_id.as_deref()
                    {
                        return Err(SOURCE_CHANGED.into());
                    }
                    ("pi:", ":message")
                }
                Header::Codex(thread_id) => {
                    if *thread_id != evidence.session_id || evidence.worker_id.is_some() {
                        return Err(SOURCE_CHANGED.into());
                    }
                    ("codex:", ":message")
                }
                Header::Claude(session_id) => {
                    if *session_id != evidence.session_id {
                        return Err(SOURCE_CHANGED.into());
                    }
                    return Ok(evidence.revision.to_string());
                }
                Header::Kiro(session_id) => {
                    if *session_id != evidence.session_id || evidence.worker_id.is_some() {
                        return Err(SOURCE_CHANGED.into());
                    }
                    ("kiro:", ":message")
                }
            };
            let member = event_id
                .strip_prefix(prefix)
                .and_then(|id| id.strip_prefix(&self.cursor.generation))
                .and_then(|id| id.strip_prefix(':'))
                .and_then(|id| id.strip_suffix(suffix))
                .ok_or(NO_EVIDENCE)?;
            match self.header {
                Header::Pi(_)
                    if evidence.revision == 0 && member == evidence.turn_id && valid_id(member) =>
                {
                    Ok(member.to_owned())
                }
                Header::Codex(_)
                    if evidence.revision == 0
                        && !member.is_empty()
                        && member.bytes().all(|byte| byte.is_ascii_digit()) =>
                {
                    Ok(member.to_owned())
                }
                Header::Kiro(_) if evidence.revision == 0 && valid_id(member) => {
                    Ok(member.to_owned())
                }
                _ => Err(NO_EVIDENCE.into()),
            }
        }

        fn rows(
            &self,
            selected: impl Iterator<Item = String>,
        ) -> Result<BTreeMap<String, RowMatch<'_>>, String> {
            let mut matches = selected
                .map(|id| (id, RowMatch::Missing))
                .collect::<BTreeMap<_, _>>();
            if matches!(self.header, Header::Codex(_) | Header::Claude(_)) {
                let minimum = if matches!(self.header, Header::Claude(_)) {
                    0
                } else {
                    self.header_len
                };
                for (offset, row) in &mut matches {
                    let Some(offset) = offset
                        .parse::<usize>()
                        .ok()
                        .filter(|offset| *offset >= minimum)
                    else {
                        continue;
                    };
                    if offset != 0 && self.bytes.get(offset - 1) != Some(&b'\n') {
                        continue;
                    }
                    let Some(line) = self.bytes.get(offset..).and_then(|tail| {
                        tail.get(..tail.len().min(MAX_ROW)).and_then(|head| {
                            head.iter()
                                .position(|byte| *byte == b'\n')
                                .map(|end| &head[..=end])
                        })
                    }) else {
                        continue;
                    };
                    *row = RowMatch::Exact {
                        line,
                        end: offset + line.len(),
                    };
                }
                return Ok(matches);
            }
            let start = if matches!(self.header, Header::Kiro(_)) {
                0
            } else {
                self.header_len
            };
            let mut end = start;
            for line in self.bytes[start..].split_inclusive(|byte| *byte == b'\n') {
                end += line.len();
                let identity = serde_json::from_slice::<RowId>(line).map_err(|_| SOURCE_CHANGED)?;
                if let Some(slot) = identity.id.as_deref().and_then(|id| matches.get_mut(id)) {
                    *slot = if matches!(slot, RowMatch::Missing) && line.len() <= MAX_ROW {
                        RowMatch::Exact { line, end }
                    } else {
                        RowMatch::Ineligible
                    };
                }
            }
            Ok(matches)
        }

        fn text(
            &self,
            evidence: &UserTextEvidence,
            event_id: &str,
            rows: &BTreeMap<String, RowMatch<'_>>,
            proof_end: usize,
        ) -> Result<String, String> {
            let id = self.row_id(evidence, event_id)?;
            let line = match rows.get(&id) {
                Some(RowMatch::Exact { line, end }) if *end <= proof_end => *line,
                Some(RowMatch::Ineligible) => {
                    return Err("Prompt preview row is ambiguous or exceeds its proof limit".into());
                }
                _ => return Err(NO_EVIDENCE.into()),
            };
            if matches!(self.header, Header::Codex(_)) {
                let row: CodexUserRow = serde_json::from_slice(line).map_err(|_| NO_EVIDENCE)?;
                if row.kind != "event_msg"
                    || row.payload.kind != "item_completed"
                    || row.payload.item.kind != "UserMessage"
                    || row.payload.thread_id != evidence.session_id
                    || row.payload.turn_id != evidence.turn_id
                    || observed_timestamp(&row.timestamp) != Some(evidence.observed_at_unix_ns)
                {
                    return Err(NO_EVIDENCE.into());
                }
                return text_preview(row.payload.item.content);
            }
            if matches!(self.header, Header::Claude(_)) {
                let row: ClaudeUserRow = serde_json::from_slice(line).map_err(|_| NO_EVIDENCE)?;
                let digest = row
                    .uuid
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .map(|id| hash(id.as_bytes())[..32].to_owned());
                let native = digest
                    .clone()
                    .unwrap_or_else(|| format!("offset-{}", evidence.revision));
                let base = format!("claude:{}:{native}", self.cursor.generation);
                let turn = digest.unwrap_or_else(|| hash(base.as_bytes())[..32].to_owned());
                if event_id != format!("{base}:message")
                    || row.kind != "user"
                    || row.session_id != evidence.session_id
                    || row.worker_id != evidence.worker_id
                    || turn != evidence.turn_id
                    || row.is_meta
                    || observed_timestamp(&row.timestamp) != Some(evidence.observed_at_unix_ns)
                {
                    return Err(NO_EVIDENCE.into());
                }
                return text_preview(row.message.content);
            }
            if matches!(self.header, Header::Kiro(_)) {
                let row: KiroUserRow = serde_json::from_slice(line).map_err(|_| NO_EVIDENCE)?;
                if row.payload.kind != "user"
                    || row.payload.source.as_deref() == Some("steer")
                    || id != evidence.turn_id
                    || observed_timestamp(&row.timestamp) != Some(evidence.observed_at_unix_ns)
                {
                    return Err(NO_EVIDENCE.into());
                }
                return text_preview(row.payload.content);
            }
            let row = serde_json::from_slice::<UserRow>(line).map_err(|_| NO_EVIDENCE)?;
            let timestamp = observed_timestamp(&row.timestamp);
            if row.kind != "message"
                || row.message.role != "user"
                || timestamp != Some(evidence.observed_at_unix_ns)
            {
                return Err(NO_EVIDENCE.into());
            }
            text_preview(row.message.content)
        }
    }

    fn observed_timestamp(value: &str) -> Option<u64> {
        OffsetDateTime::parse(value, &Rfc3339)
            .ok()
            .and_then(|time| u64::try_from(time.unix_timestamp_nanos()).ok())
    }

    enum Header {
        Pi(PiHeader),
        Codex(String),
        Claude(String),
        Kiro(String),
    }

    #[derive(Deserialize)]
    struct CodexHeader {
        #[serde(rename = "type")]
        kind: String,
        payload: CodexHeaderPayload,
    }

    #[derive(Deserialize)]
    struct CodexHeaderPayload {
        id: String,
        session_id: Option<String>,
        source: Option<serde_json::Value>,
    }

    #[derive(Deserialize)]
    struct CodexUserRow {
        #[serde(rename = "type")]
        kind: String,
        timestamp: String,
        payload: CodexUserPayload,
    }

    #[derive(Deserialize)]
    struct CodexUserPayload {
        #[serde(rename = "type")]
        kind: String,
        thread_id: String,
        turn_id: String,
        item: CodexUserItem,
    }

    #[derive(Deserialize)]
    struct CodexUserItem {
        #[serde(rename = "type")]
        kind: String,
        content: UserContent,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ClaudeHeader {
        session_id: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ClaudeUserRow {
        #[serde(rename = "type")]
        kind: String,
        uuid: Option<String>,
        session_id: String,
        #[serde(rename = "agentId")]
        worker_id: Option<String>,
        #[serde(default)]
        is_meta: bool,
        timestamp: String,
        message: ClaudeMessage,
    }

    #[derive(Deserialize)]
    struct ClaudeMessage {
        content: UserContent,
    }

    #[derive(Deserialize)]
    struct KiroUserRow {
        timestamp: String,
        payload: KiroUserPayload,
    }

    #[derive(Deserialize)]
    struct KiroUserPayload {
        #[serde(rename = "type")]
        kind: String,
        source: Option<String>,
        content: UserContent,
    }

    #[derive(Deserialize)]
    struct PiHeader {
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
            return Err("Content is unavailable within the preview text limit".into());
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
                "Prompt preview withheld because it contains possible credential material".into(),
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
            return Err("Content is unavailable within the preview text limit".into());
        }
        Ok(text)
    }

    fn open_member(path: &Path, harness: Harness) -> Result<File, String> {
        let unavailable = "cannot open the configured source member without following links";
        let mut components = path.components();
        if components.next() != Some(Component::RootDir)
            || (harness != Harness::OpenCode
                && path.extension().and_then(|extension| extension.to_str()) != Some("jsonl"))
        {
            return Err("Content preview requires one configured JSONL file".into());
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
