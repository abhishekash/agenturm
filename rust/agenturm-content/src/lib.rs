//! Shared source proof and fenced private-content delivery.
#![cfg(target_os = "linux")]

use agenturm_jsonl_source::{CursorProof, cursor_proof};
use agenturm_kernel::ExtensionId;
use agenturm_store::{
    CodexOperationEvidence, CodexOperationSelection, ContentGrantScope, ContentKind, Store,
    UserTextEvidence,
};
use rustix::fs::{Mode, OFlags, open, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{Read, Write};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
const PI_PLUGIN_ID: &str = "example.rust-pi-harness";
const CODEX_PLUGIN_ID: &str = "example.rust-codex-harness";
const CLAUDE_PLUGIN_ID: &str = "example.rust-claude-harness";
const KIRO_PLUGIN_ID: &str = "example.rust-kiro-harness";
const OPENCODE_PLUGIN_ID: &str = "example.rust-opencode-harness";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationDetailKind {
    Mcp,
    Edit,
}
/// Broker-owned resolved source inputs. Clients never supply these paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedSource {
    pub source_id: String,
    pub plugin_id: String,
    pub plugin_host: PathBuf,
    pub adapter: PathBuf,
    pub artifact_sha256: Option<String>,
    pub features: Option<Vec<String>>,
    pub source_root: PathBuf,
}
pub type SourceResolver<'a> = dyn Fn(&ExtensionId) -> Result<ResolvedSource, String> + 'a;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptReference {
    pub event_id: String,
    pub revision: u32,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedPrompt {
    pub text: String,
    pub session_id: String,
    pub worker_id: Option<String>,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
    pub grant_binding: [u8; 32],
}
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
const SOURCE_CHANGED: &str = "Prompt source changed or no longer matches the committed checkpoint";
const NO_EVIDENCE: &str = "selected revision is unavailable in the current source commit";

struct OperationDefinition {
    grant: ContentKind,
    fingerprint_domain: &'static [u8],
    event_suffix: &'static str,
    native_kind: &'static str,
    description: &'static str,
    unavailable: &'static str,
}

fn operation_definition(kind: OperationDetailKind) -> OperationDefinition {
    match kind {
        OperationDetailKind::Mcp => OperationDefinition {
            grant: ContentKind::CodexMcpPayload,
            fingerprint_domain: b"agenturm.codex-mcp-payload/1",
            event_suffix: ":mcp-call",
            native_kind: "McpToolCall",
            description: "MCP payload",
            unavailable: "The selected MCP operation has no verified payload",
        },
        OperationDetailKind::Edit => OperationDefinition {
            grant: ContentKind::CodexEditDetail,
            fingerprint_domain: b"agenturm.codex-edit-detail/1",
            event_suffix: ":file-edit",
            native_kind: "FileChange",
            description: "file edit",
            unavailable: "The selected file edit has no verified detail",
        },
    }
}

pub fn list(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    session_id: &str,
) -> Result<Vec<PromptReference>, String> {
    if !valid_id(session_id) {
        return Err("invalid selected session ID".into());
    }
    store
        .with_user_content_fence(|store| {
            let mut source = BoundSource::open(resolver, source_id)?;
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
            let fresh = source.recheck(resolver, source_id)?;
            store
                .verify_content_grant(&fresh.scope, &grant)
                .map_err(|error| error.to_string())?;
            fresh.verify_additional_checkpoint(&first.cursor)?;
            Ok(references)
        })
        .map_err(|error| error.to_string())?
}

pub fn reveal_fenced(
    writer: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    session_id: Option<&str>,
    event_id: &str,
    revision: u32,
    output: &mut dyn Write,
) -> Result<(), String> {
    with_verified_prompt(
        writer,
        resolver,
        source_id,
        session_id,
        event_id,
        revision,
        |prompt| {
            // JSON escaping keeps controls out of terminal output. The source
            // validator has already removed C1 and Unicode direction controls.
            let mut line =
                serde_json::to_vec(&prompt.text).map_err(|_| "cannot encode user-text preview")?;
            line.push(b'\n');
            write_output(output, &line)
        },
    )?
}

/// Read exact native evidence and consume it while the current approval and
/// retained store revision are serialized against revocation and retention.
pub fn with_verified_prompt<T>(
    writer: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    session_id: Option<&str>,
    event_id: &str,
    revision: u32,
    consume: impl FnOnce(&VerifiedPrompt) -> T,
) -> Result<T, String> {
    writer
        .with_user_content_fence(|store| {
            let prompt =
                verified_prompt(store, resolver, source_id, session_id, event_id, revision)?;
            Ok(consume(&prompt))
        })
        .map_err(|error| error.to_string())?
}

pub fn reveal_operation_fenced(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    kind: OperationDetailKind,
    selection: &CodexOperationSelection<'_>,
    output: &mut dyn Write,
    terminal: bool,
) -> Result<(), String> {
    if !valid_id(selection.session_id)
        || !valid_id(selection.operation_id)
        || selection.turn_id.is_some_and(|turn| !valid_id(turn))
    {
        return Err("Invalid session, turn, or operation ID".into());
    }
    let operation = operation_definition(kind);
    store
        .with_user_content_fence(|store| {
            let mut source = BoundSource::open_with_kind(resolver, source_id, Some(kind))?;
            let grant = store
                .current_content_grant(&source.scope)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!(
                        "Codex {} preview requires approval for the current source",
                        operation.description
                    )
                })?;
            let select = |scope: &ContentGrantScope| {
                store
                    .current_codex_operation_evidence(scope, &grant, selection)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| operation.unavailable.to_owned())
            };
            let evidence = select(&source.scope)?;
            let prefix = source.verified_prefix(&evidence.cursor)?;
            let payload = prefix.operation_payload(kind, &operation, selection, &evidence)?;
            let mut reveal = payload
                .encode(terminal)
                .map_err(|_| operation.unavailable)?;
            let fresh = source.recheck(resolver, source_id)?;
            if select(&fresh.scope)? != evidence {
                return Err("The selected operation changed".into());
            }
            reveal.push('\n');
            write_output(output, reveal.as_bytes())
        })
        .map_err(|error| error.to_string())?
}

fn verified_prompt(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    session_id: Option<&str>,
    event_id: &str,
    revision: u32,
) -> Result<VerifiedPrompt, String> {
    if event_id.is_empty() || event_id.len() > 256 {
        return Err("invalid prompt preview event ID".into());
    }
    if session_id.is_some_and(|id| !valid_id(id)) {
        return Err("invalid selected session ID".into());
    }
    let mut source = BoundSource::open(resolver, source_id)?;
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
    let fresh = source.recheck(resolver, source_id)?;
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
    Ok(VerifiedPrompt {
        text,
        session_id: evidence.session_id,
        worker_id: evidence.worker_id,
        turn_id: evidence.turn_id,
        observed_at_unix_ns: evidence.observed_at_unix_ns,
        grant_binding: grant.binding_fingerprint(),
    })
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

pub struct BoundSource {
    file: File,
    stamp: FileStamp,
    scope: ContentGrantScope,
    harness: Harness,
    source_path: PathBuf,
    operation_kind: Option<OperationDetailKind>,
}

impl BoundSource {
    fn open(resolver: &SourceResolver<'_>, source_id: &ExtensionId) -> Result<Self, String> {
        Self::open_with_kind(resolver, source_id, None)
    }

    fn open_with_kind(
        resolver: &SourceResolver<'_>,
        source_id: &ExtensionId,
        operation_kind: Option<OperationDetailKind>,
    ) -> Result<Self, String> {
        let source = resolver(source_id)?;
        if source.source_id != source_id.as_str() {
            return Err("Content resolver returned a different source".into());
        }
        let harness = match source.plugin_id.as_str() {
            PI_PLUGIN_ID => Harness::Pi,
            CODEX_PLUGIN_ID => Harness::Codex,
            CLAUDE_PLUGIN_ID => Harness::Claude,
            KIRO_PLUGIN_ID => Harness::Kiro,
            OPENCODE_PLUGIN_ID => Harness::OpenCode,
            _ => {
                return Err("Prompt preview requires a supported pinned source".into());
            }
        };
        if operation_kind.is_some() && harness != Harness::Codex {
            return Err("Operation details require a supported pinned Codex source".into());
        }
        let artifact = source
            .artifact_sha256
            .as_deref()
            .ok_or("Prompt preview requires a SHA-256-pinned adapter in catalog configuration")?;
        let file = open_member(&source.source_root, harness)?;
        let stamp = FileStamp::from_file(&file)?;
        let feature_set = source.features.as_deref().unwrap_or_default();
        let mut features = feature_set.iter().map(String::as_bytes).collect::<Vec<_>>();
        features.sort_unstable();
        let device = stamp.device.to_be_bytes();
        let inode = stamp.inode.to_be_bytes();
        let mut fingerprint = Sha256::new();
        let operation = operation_kind.map(operation_definition);
        let domain = operation.as_ref().map_or_else(
            || harness.fingerprint_domain(),
            |operation| operation.fingerprint_domain,
        );
        for field in [
            domain,
            source.source_id.as_bytes(),
            source.plugin_id.as_bytes(),
            artifact.as_bytes(),
            source.plugin_host.as_os_str().as_bytes(),
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
            operation.map_or_else(|| harness.content_kind(), |operation| operation.grant),
            fingerprint.finalize().into(),
        );
        Ok(Self {
            file,
            stamp,
            scope,
            harness,
            source_path: source.source_root.clone(),
            operation_kind,
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
                        admitted_proof: row.admitted_cursor.as_deref(),
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
            return Err("Prompt preview requires the source's initial verified generation".into());
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

    fn recheck(
        &self,
        resolver: &SourceResolver<'_>,
        source_id: &ExtensionId,
    ) -> Result<Self, String> {
        let fresh = Self::open_with_kind(resolver, source_id, self.operation_kind)?;
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
    fn operation_payload(
        &self,
        kind: OperationDetailKind,
        operation: &OperationDefinition,
        selection: &CodexOperationSelection<'_>,
        evidence: &CodexOperationEvidence,
    ) -> Result<CodexOperationPayload, String> {
        let unavailable = operation.unavailable;
        if !matches!(&self.header, Header::Codex(thread) if thread == selection.session_id)
            || selection.revision != 0
        {
            return Err(unavailable.into());
        }
        let offset = selection
            .event_id
            .strip_prefix("codex:")
            .and_then(|event| event.strip_prefix(&self.cursor.generation))
            .and_then(|event| event.strip_prefix(':'))
            .and_then(|event| event.strip_suffix(operation.event_suffix))
            .filter(|offset| !offset.is_empty() && offset.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or(unavailable)?;
        let proofs = self.admitted_offsets(std::iter::once(evidence.admitted_cursor.as_str()));
        let proof_end = proofs
            .get(evidence.admitted_cursor.as_str())
            .ok_or(unavailable)?;
        let rows = self.rows(std::iter::once(offset.to_owned()))?;
        let line = match rows.get(offset) {
            Some(RowMatch::Exact { line, end }) if end <= proof_end => *line,
            _ => return Err(unavailable.into()),
        };
        let row: CodexOperationRow = serde_json::from_slice(line).map_err(|_| unavailable)?;
        let item = row.payload.item;
        if row.kind != "event_msg"
            || row.payload.kind != "item_completed"
            || row.payload.thread_id != selection.session_id
            || row.payload.turn_id != evidence.turn_id
            || item.kind != operation.native_kind
            || item.id.is_empty()
            || hash(item.id.as_bytes()) != selection.operation_id
            || observed_timestamp(&row.timestamp) != Some(evidence.observed_at_unix_ns)
        {
            return Err(unavailable.into());
        }
        match kind {
            OperationDetailKind::Mcp => item.mcp_detail(),
            OperationDetailKind::Edit => item.edit_detail(),
        }
        .ok_or_else(|| unavailable.to_owned())
    }

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
            Header::Kiro(_) if evidence.revision == 0 && valid_id(member) => Ok(member.to_owned()),
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
struct CodexOperationRow {
    #[serde(rename = "type")]
    kind: String,
    timestamp: String,
    payload: CodexItemCompleted,
}

#[derive(Deserialize)]
struct CodexItemCompleted {
    #[serde(rename = "type")]
    kind: String,
    thread_id: String,
    turn_id: String,
    item: CodexOperationItem,
}

#[derive(Deserialize)]
struct CodexOperationItem {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    #[serde(default, deserialize_with = "present_payload_value")]
    arguments: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_payload_value")]
    result: Option<serde_json::Value>,
    changes: Option<BTreeMap<String, NativeFileChange>>,
}

impl CodexOperationItem {
    fn mcp_detail(self) -> Option<CodexOperationPayload> {
        let fields = [("arguments", self.arguments), ("result", self.result)]
            .into_iter()
            .filter_map(|(field, value)| value.map(|value| (field.to_owned(), value)))
            .collect::<serde_json::Map<_, _>>();
        let text = serde_json::to_string(&fields).ok()?;
        if fields.is_empty() || text_preview(UserContent::Text(text.clone())).ok()? != text {
            return None;
        }
        Some(CodexOperationPayload::Mcp(fields))
    }

    fn edit_detail(self) -> Option<CodexOperationPayload> {
        let changes = self.changes?;
        if !(1..=agenturm_kernel::FileEdit::MAX_CHANGES).contains(&changes.len()) {
            return None;
        }
        let changes = changes
            .into_iter()
            .map(|(path, edit)| {
                if !edit_path(&path)
                    || edit
                        .move_path
                        .as_deref()
                        .is_some_and(|path| !edit_path(path))
                    || edit
                        .diff
                        .as_deref()
                        .is_some_and(|text| !safe_edit_text(text))
                    || edit
                        .content
                        .as_deref()
                        .is_some_and(|text| !safe_edit_text(text))
                {
                    return None;
                }
                Some(EditChange { path, edit })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(CodexOperationPayload::Edit { changes })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum EditAction {
    Add,
    Delete,
    Update,
}

#[derive(Deserialize, Serialize)]
struct NativeFileChange {
    #[serde(rename = "type")]
    action: EditAction,
    #[serde(alias = "unified_diff", skip_serializing_if = "Option::is_none")]
    diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    move_path: Option<String>,
}

#[derive(Serialize)]
struct EditChange {
    path: String,
    #[serde(flatten)]
    edit: NativeFileChange,
}

#[derive(Serialize)]
#[serde(untagged)]
enum CodexOperationPayload {
    Mcp(serde_json::Map<String, serde_json::Value>),
    Edit { changes: Vec<EditChange> },
}

impl CodexOperationPayload {
    fn encode(&self, terminal: bool) -> Result<String, ()> {
        let encoded = serde_json::to_string(self).map_err(|_| ())?;
        if encoded.len() > MAX_ROW {
            return Err(());
        }
        if !terminal {
            return Ok(encoded);
        }
        let display = match self {
            Self::Mcp(fields) => serde_json::to_string_pretty(fields).map_err(|_| ())?,
            Self::Edit { changes } => changes
                .iter()
                .map(EditChange::display)
                .collect::<Vec<_>>()
                .join("\n\n"),
        };
        serde_json::to_string(&display).map_err(|_| ())
    }
}

impl EditChange {
    fn display(&self) -> String {
        let action = match self.edit.action {
            EditAction::Add => "Added",
            EditAction::Delete => "Deleted",
            EditAction::Update if self.edit.move_path.is_some() => "Moved",
            EditAction::Update => "Updated",
        };
        let mut text = format!("{action} {}", self.path);
        if let Some(path) = &self.edit.move_path {
            text.push_str(" → ");
            text.push_str(path);
        }
        for body in [self.edit.diff.as_deref(), self.edit.content.as_deref()]
            .into_iter()
            .flatten()
        {
            text.push('\n');
            text.push_str(body);
        }
        text
    }
}

fn edit_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && !path.chars().any(char::is_control)
        && safe_edit_text(path)
}

fn safe_edit_text(text: &str) -> bool {
    text.is_empty()
        || bounded_text_preview(text.to_owned(), MAX_ROW)
            .ok()
            .as_deref()
            == Some(text)
}

fn present_payload_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
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
    bounded_text_preview(text, MAX_TEXT_BYTES)
}

fn bounded_text_preview(text: String, max_bytes: usize) -> Result<String, String> {
    if text.is_empty() || text.len() > max_bytes {
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
        words.next() == Some("mysql") && words.any(|word| word.starts_with("-p") && word.len() > 2)
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
    if known_credential || password_command || bearer_value || assigned_credential || uri_credential
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
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
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

/// Opens the exact configured source and computes the existing grant binding.
pub fn bind(
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    kind: Option<OperationDetailKind>,
) -> Result<BoundSource, String> {
    BoundSource::open_with_kind(resolver, source_id, kind)
}
impl BoundSource {
    pub fn scope(&self) -> &ContentGrantScope {
        &self.scope
    }
    pub fn label(&self) -> &'static str {
        self.harness.label()
    }
}

pub use reveal_fenced as reveal_user;
pub use reveal_operation_fenced as reveal_operation;
