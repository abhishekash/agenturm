//! Shared source proof and fenced private-content delivery.
#![cfg(target_os = "linux")]

mod command_failure;

use agenturm_jsonl_source::{CursorProof, cursor_proof};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use agenturm_opencode_harness::PromptSourceStamp;
use agenturm_store::{
    CodexOperationEvidence, CodexOperationSelection, ContentGrantScope, ContentKind, Store,
    UserTextEvidence, bounded_text_preview,
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
    Command,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<ReadSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_identity: Option<String>,
}

pub struct PromptSelection<'a> {
    pub session_id: Option<&'a str>,
    pub event_id: &'a str,
    pub revision: u32,
    pub snapshot_id: Option<ReadSnapshot>,
    pub history_identity: Option<&'a str>,
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedEdit {
    pub changes: Vec<EditChange>,
    /// Provider-specific evidence is never coerced into Codex add/update/delete
    /// diffs. The optional field preserves the existing Codex proof shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_detail: Option<ProviderEditDetail>,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
    pub grant_binding: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "provider",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProviderEditDetail {
    Pi(PiEditDetail),
    Claude(ClaudeEditDetail),
    OpenCode(OpenCodeEditDetail),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedCommand {
    pub recorded_invocation: RecordedInvocation,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
    pub grant_binding: [u8; 32],
}

/// Native strings and argument vectors retain their original representation.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum RecordedInvocation {
    Text(String),
    Argv(Vec<String>),
}

impl RecordedInvocation {
    pub fn searchable_text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Text(text) => std::borrow::Cow::Borrowed(text),
            Self::Argv(args) => std::borrow::Cow::Owned(args.join(" ")),
        }
    }
}

impl VerifiedEdit {
    /// Compare recorded paths literally, including the destination of a move.
    pub fn contains_path(&self, path: &str) -> bool {
        match &self.provider_detail {
            Some(ProviderEditDetail::Pi(detail)) => detail.path == path,
            Some(ProviderEditDetail::Claude(detail)) => detail.file_path == path,
            Some(ProviderEditDetail::OpenCode(detail)) => {
                !detail.truncated && detail.files.iter().any(|file| file == path)
            }
            None => self.changes.iter().any(|change| {
                change.path == path || change.edit.move_path.as_deref() == Some(path)
            }),
        }
    }

    /// Deliver the verified edit using the private terminal preview format.
    pub fn write_terminal(&self, output: &mut dyn Write) -> Result<(), String> {
        let display = match &self.provider_detail {
            Some(ProviderEditDetail::Pi(detail)) => detail.display(),
            Some(ProviderEditDetail::Claude(detail)) => detail.display(),
            Some(ProviderEditDetail::OpenCode(detail)) => detail.display(),
            None => display_edit_changes(&self.changes),
        };
        let mut text =
            serde_json::to_string(&display).map_err(|_| "Cannot encode the verified file edit")?;
        text.push('\n');
        write_output(output, text.as_bytes())
    }
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
/// Per-string bound for revealed Claude edit detail, matching the shared
/// user-text preview limit.
const MAX_EDIT_TEXT_BYTES: usize = MAX_TEXT_BYTES;
/// The Claude adapter withholds translation of any message whose block
/// count exceeds this budget, so a larger tool_use index cannot exist.
const MAX_CLAUDE_BLOCKS: usize = 128;
const SOURCE_CHANGED: &str = "Prompt source changed or no longer matches the committed checkpoint";
const NO_EVIDENCE: &str = "selected revision is unavailable in the current source commit";

struct OperationDefinition {
    grant: ContentKind,
    fingerprint_domain: &'static [u8],
    event_suffix: &'static str,
    native_kind: &'static str,
    description: &'static str,
    unavailable: &'static str,
    /// Harness label used for approval-facing messages.
    label: &'static str,
}

/// The exact per-harness contract for one operation-detail kind. Each
/// harness pins its own grant kind, fingerprint domain, and native row
/// shape; unsupported combinations deny with an explicit message instead
/// of borrowing another harness's evidence.
fn operation_definition(
    harness: Harness,
    kind: OperationDetailKind,
) -> Result<OperationDefinition, String> {
    match (harness, kind) {
        (Harness::Codex, OperationDetailKind::Mcp) => Ok(OperationDefinition {
            grant: ContentKind::CodexMcpPayload,
            fingerprint_domain: b"agenturm.codex-mcp-payload/1",
            event_suffix: ":mcp-call",
            native_kind: "McpToolCall",
            description: "MCP payload",
            unavailable: "The selected MCP operation has no verified payload",
            label: "Codex",
        }),
        (Harness::Codex, OperationDetailKind::Edit) => Ok(OperationDefinition {
            grant: ContentKind::CodexEditDetail,
            fingerprint_domain: b"agenturm.codex-edit-detail/1",
            event_suffix: ":file-edit",
            native_kind: "FileChange",
            description: "file edit",
            unavailable: "The selected file edit has no verified detail",
            label: "Codex",
        }),
        (Harness::Codex, OperationDetailKind::Command) => Ok(OperationDefinition {
            grant: ContentKind::CodexCommandDetail,
            fingerprint_domain: b"agenturm.codex-command-detail/1",
            event_suffix: ":tool",
            native_kind: "CommandExecution",
            description: "recorded command invocation",
            unavailable: "The selected command has no verified invocation",
            label: "Codex",
        }),
        (Harness::Pi, OperationDetailKind::Edit) => Ok(OperationDefinition {
            grant: ContentKind::PiEditDetail,
            fingerprint_domain: b"agenturm.pi-edit-detail/1",
            event_suffix: ":file-change-",
            native_kind: "toolCall",
            description: "file edit",
            unavailable: "The selected Pi file edit has no verified detail",
            label: "Pi",
        }),
        (Harness::Claude, OperationDetailKind::Edit) => Ok(OperationDefinition {
            grant: ContentKind::ClaudeEditDetail,
            fingerprint_domain: b"agenturm.claude-edit-detail/1",
            event_suffix: ":file-edit-",
            native_kind: "tool_use",
            description: "file edit",
            unavailable: "The selected Claude file edit has no verified detail",
            label: "Claude Code",
        }),
        (Harness::Claude, OperationDetailKind::Mcp) => Ok(OperationDefinition {
            grant: ContentKind::ClaudeMcpPayload,
            fingerprint_domain: b"agenturm.claude-mcp-payload/1",
            event_suffix: ":mcp-use-",
            native_kind: "tool_use",
            description: "MCP payload",
            unavailable: "The selected Claude MCP operation has no verified payload",
            label: "Claude Code",
        }),
        (Harness::Claude, OperationDetailKind::Command) => {
            Err("Command detail preview requires a supported pinned Codex source".into())
        }
        (Harness::OpenCode, OperationDetailKind::Edit) => Ok(OperationDefinition {
            grant: ContentKind::OpenCodeEditDetail,
            fingerprint_domain: b"agenturm.opencode-edit-detail/1",
            event_suffix: ":file-change",
            native_kind: "patch",
            description: "file edit",
            unavailable: "The selected OpenCode file change has no verified detail",
            label: "OpenCode",
        }),
        (Harness::OpenCode, OperationDetailKind::Mcp) => {
            Err("MCP payload preview requires a supported pinned Codex source".into())
        }
        (Harness::OpenCode, OperationDetailKind::Command) => {
            Err("Command detail preview requires a supported pinned Codex source".into())
        }
        _ => Err(
            "Operation details require a supported pinned Pi, Codex, Claude, or OpenCode source"
                .into(),
        ),
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
            let identity = store
                .history_identity()
                .map_err(|error| error.to_string())?;

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
                        snapshot_id: Some(evidence.snapshot_id),
                        history_identity: Some(identity.clone()),
                    });
                }
            }
            let fresh = source.recheck(resolver, source_id)?;
            store
                .verify_content_grant(&fresh.scope, &grant)
                .map_err(|error| error.to_string())?;
            source.verify_additional_checkpoint()?;
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
    reveal_selected_prompt(
        writer,
        resolver,
        source_id,
        PromptSelection {
            session_id,
            event_id,
            revision,
            snapshot_id: None,
            history_identity: None,
        },
        output,
    )
}

pub fn reveal_selected_prompt(
    writer: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: PromptSelection<'_>,
    output: &mut dyn Write,
) -> Result<(), String> {
    with_selected_prompt(writer, resolver, source_id, selection, |prompt| {
        // JSON escaping keeps controls out of terminal output. The source
        // validator has already removed C1 and Unicode direction controls.
        let mut line =
            serde_json::to_vec(&prompt.text).map_err(|_| "cannot encode user-text preview")?;
        line.push(b'\n');
        write_output(output, &line)
    })?
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
    with_selected_prompt(
        writer,
        resolver,
        source_id,
        PromptSelection {
            session_id,
            event_id,
            revision,
            snapshot_id: None,
            history_identity: None,
        },
        consume,
    )
}

pub fn with_selected_prompt<T>(
    writer: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: PromptSelection<'_>,
    consume: impl FnOnce(&VerifiedPrompt) -> T,
) -> Result<T, String> {
    writer
        .with_user_content_fence(|store| {
            let prompt = verified_prompt(store, resolver, source_id, selection)?;
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
    with_verified_operation(
        store,
        resolver,
        source_id,
        kind,
        selection,
        |operation, payload, _, _| {
            let mut reveal = payload
                .encode(terminal)
                .map_err(|_| operation.unavailable)?;
            reveal.push('\n');
            write_output(output, reveal.as_bytes())
        },
    )
}

/// Uses one exact retained edit while its provider-specific approval and
/// evidence are fenced. Path-only patches and tool inputs retain their own
/// shape instead of being promoted to Codex diffs.
pub fn with_verified_edit<T>(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: &CodexOperationSelection<'_>,
    consume: impl FnOnce(&VerifiedEdit) -> T,
) -> Result<T, String> {
    with_verified_operation(
        store,
        resolver,
        source_id,
        OperationDetailKind::Edit,
        selection,
        |operation, payload, evidence, grant_binding| {
            payload.encode(false).map_err(|_| operation.unavailable)?;
            let (changes, provider_detail) = match payload {
                OperationPayload::Edit { changes } => (changes, None),
                OperationPayload::PiEdit(detail) => {
                    (Vec::new(), Some(ProviderEditDetail::Pi(detail)))
                }
                OperationPayload::ClaudeEdit(detail) => {
                    (Vec::new(), Some(ProviderEditDetail::Claude(detail)))
                }
                OperationPayload::OpenCodeEdit(detail) => {
                    (Vec::new(), Some(ProviderEditDetail::OpenCode(detail)))
                }
                _ => return Err(operation.unavailable.into()),
            };
            Ok(consume(&VerifiedEdit {
                changes,
                provider_detail,
                turn_id: evidence.turn_id,
                observed_at_unix_ns: evidence.observed_at_unix_ns,
                grant_binding,
            }))
        },
    )
}

pub fn with_verified_command<T>(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: &CodexOperationSelection<'_>,
    consume: impl FnOnce(&VerifiedCommand) -> T,
) -> Result<T, String> {
    with_verified_command_matching(store, resolver, source_id, selection, false, consume)
}

/// Apply terminal-result rules while the exact admitted command record and its
/// independent content grant are fenced.
pub fn with_verified_failed_command<T>(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: &CodexOperationSelection<'_>,
    consume: impl FnOnce(&VerifiedCommand) -> T,
) -> Result<T, String> {
    with_verified_command_matching(store, resolver, source_id, selection, true, consume)
}

fn with_verified_command_matching<T>(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: &CodexOperationSelection<'_>,
    failed: bool,
    consume: impl FnOnce(&VerifiedCommand) -> T,
) -> Result<T, String> {
    with_verified_operation(
        store,
        resolver,
        source_id,
        OperationDetailKind::Command,
        selection,
        |operation, payload, evidence, grant_binding| {
            let OperationPayload::Command {
                recorded_invocation,
                confirmed_failure,
            } = payload
            else {
                return Err(operation.unavailable.into());
            };
            if failed && !confirmed_failure {
                return Err(operation.unavailable.into());
            }
            Ok(consume(&VerifiedCommand {
                recorded_invocation,
                turn_id: evidence.turn_id,
                observed_at_unix_ns: evidence.observed_at_unix_ns,
                grant_binding,
            }))
        },
    )
}

fn with_verified_operation<T>(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    kind: OperationDetailKind,
    selection: &CodexOperationSelection<'_>,
    consume: impl FnOnce(
        &OperationDefinition,
        OperationPayload,
        CodexOperationEvidence,
        [u8; 32],
    ) -> Result<T, String>,
) -> Result<T, String> {
    if !valid_id(selection.session_id)
        || !valid_operation_id(selection.operation_id)
        || selection.turn_id.is_some_and(|turn| !valid_id(turn))
    {
        return Err("Invalid session, turn, or operation ID".into());
    }
    store
        .with_user_content_fence(|store| {
            let mut source = BoundSource::open_with_kind(resolver, source_id, Some(kind))?;
            let operation = source.operation_definition(kind)?;
            let grant = store
                .current_content_grant(&source.scope)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!(
                        "{} {} preview requires approval for the current source",
                        operation.label, operation.description
                    )
                })?;
            let select = |scope: &ContentGrantScope| {
                store
                    .current_codex_operation_evidence(scope, &grant, selection)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| operation.unavailable.to_owned())
            };
            let evidence = select(&source.scope)?;
            let payload = if source.harness == Harness::OpenCode {
                source.opencode_edit_payload(&operation, selection, &evidence)?
            } else {
                let prefix = source.verified_prefix(&evidence.cursor)?;
                prefix.operation_payload(kind, &operation, selection, &evidence)?
            };
            let fresh = source.recheck(resolver, source_id)?;
            if select(&fresh.scope)? != evidence {
                return Err("The selected operation changed".into());
            }
            if source.harness == Harness::OpenCode {
                source.verify_additional_checkpoint()?;
            }
            consume(&operation, payload, evidence, grant.binding_fingerprint())
        })
        .map_err(|error| error.to_string())?
}

fn verified_prompt(
    store: &Store,
    resolver: &SourceResolver<'_>,
    source_id: &ExtensionId,
    selection: PromptSelection<'_>,
) -> Result<VerifiedPrompt, String> {
    let PromptSelection {
        session_id,
        event_id,
        revision,
        snapshot_id,
        history_identity,
    } = selection;
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
    match (snapshot_id, history_identity) {
        (None, None) => {}
        (Some(snapshot), Some(identity)) => {
            if store
                .history_identity()
                .map_err(|error| error.to_string())?
                != identity
            {
                return Err("The prompt reference belongs to different history".into());
            }
            if !store
                .contains_observation_at_snapshot(source_id, event_id, revision, snapshot)
                .map_err(|error| error.to_string())?
            {
                return Err(NO_EVIDENCE.into());
            }
        }
        _ => return Err("The selected prompt reference is incomplete".into()),
    }
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
    source.verify_additional_checkpoint()?;
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
    opencode_stamp: Option<PromptSourceStamp>,
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
        if operation_kind.is_some()
            && !matches!(
                harness,
                Harness::Pi | Harness::Codex | Harness::Claude | Harness::OpenCode
            )
        {
            return Err(
                "Operation details require a supported pinned Pi, Codex, Claude, or OpenCode source"
                    .into(),
            );
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
        let operation = operation_kind
            .map(|kind| operation_definition(harness, kind))
            .transpose()?;
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
            opencode_stamp: None,
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
            let proven = agenturm_opencode_harness::prove_user_prompts(
                &self.source_path,
                &first.cursor,
                &first.session_id,
                &candidates,
            )
            .map_err(|error| error.to_string())?;
            self.opencode_stamp = Some(proven.stamp);
            let texts = proven
                .texts
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

    fn operation_definition(
        &self,
        kind: OperationDetailKind,
    ) -> Result<OperationDefinition, String> {
        operation_definition(self.harness, kind)
    }

    /// Verifies the exact native OpenCode patch part behind the selected
    /// file-change occurrence and extracts its bounded edited-path list. The
    /// pinned database is re-read in a read-only transaction; the part and
    /// message identity, the observed timestamp, the canonical record digest, and the
    /// file count must all match the retained observation exactly before any
    /// path is disclosed. Paths are truncated to the shared edit-text bound
    /// and screened; unsafe text withholds the entire payload.
    fn opencode_edit_payload(
        &mut self,
        operation: &OperationDefinition,
        selection: &CodexOperationSelection<'_>,
        evidence: &CodexOperationEvidence,
    ) -> Result<OperationPayload, String> {
        let unavailable = operation.unavailable;
        let (Some(digest), Some(files)) = (evidence.digest.as_deref(), evidence.files) else {
            return Err(unavailable.into());
        };
        let proven = agenturm_opencode_harness::prove_patch_part(
            &self.source_path,
            &evidence.cursor,
            &agenturm_opencode_harness::PatchPartEvidence {
                event_id: selection.event_id,
                operation_id: selection.operation_id,
                turn_id: &evidence.turn_id,
                session_id: selection.session_id,
                observed_at_unix_ns: evidence.observed_at_unix_ns,
                digest,
                files,
            },
        )
        .map_err(|error| {
            // The harness reports a changed pinned source with its own exact
            // message; every other failure is an unavailable detail.
            if error.to_string()
                == "OpenCode edit source changed or no longer matches the committed checkpoint"
            {
                SOURCE_CHANGED.to_owned()
            } else {
                unavailable.to_owned()
            }
        })?;
        self.opencode_stamp = Some(proven.stamp);
        // Each native path is truncated to the shared 512-byte edit-path
        // bound on a character boundary; any path that is still unsafe
        // withholds the entire payload rather than disclosing a neighbor.
        const MAX_PATH_BYTES: usize = 512;
        let mut paths = Vec::new();
        let mut truncated = false;
        let mut bytes = 0usize;
        for path in proven.files {
            let (prefix, cut) = if path.len() <= MAX_PATH_BYTES {
                (path.as_str(), false)
            } else {
                let mut end = MAX_PATH_BYTES;
                while end > 0 && !path.is_char_boundary(end) {
                    end -= 1;
                }
                (&path[..end], true)
            };
            if !edit_path(prefix) {
                return Err(unavailable.into());
            }
            let encoded_bytes = serde_json::to_string(prefix)
                .map_err(|_| unavailable)?
                .len()
                + 1;
            // Reserve object keys, count, and the truncation marker as well as
            // the JSON escaping cost of each displayed path.
            if bytes + encoded_bytes + 256 > MAX_ROW {
                truncated = true;
                break;
            }
            bytes += encoded_bytes;
            truncated |= cut;
            paths.push(prefix.to_owned());
        }
        Ok(OperationPayload::OpenCodeEdit(OpenCodeEditDetail {
            files: paths,
            reported_files: files,
            truncated,
        }))
    }

    fn verify_additional_checkpoint(&self) -> Result<(), String> {
        if self.harness == Harness::OpenCode {
            self.opencode_stamp
                .as_ref()
                .ok_or(SOURCE_CHANGED)?
                .verify(&self.source_path)
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
                    || header.payload.thread_source.as_deref() == Some("guardian_review")
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
    ) -> Result<OperationPayload, String> {
        match &self.header {
            Header::Codex(_) => self.codex_operation_payload(operation, selection, evidence),
            Header::Claude(_) if kind == OperationDetailKind::Mcp => {
                self.claude_mcp_payload(operation, selection, evidence)
            }
            Header::Pi(_) if kind == OperationDetailKind::Edit => {
                self.pi_edit_payload(operation, selection, evidence)
            }
            Header::Claude(_) if kind == OperationDetailKind::Edit => {
                self.claude_edit_payload(operation, selection, evidence)
            }
            _ => Err(operation.unavailable.into()),
        }
    }

    fn codex_operation_payload(
        &self,
        operation: &OperationDefinition,
        selection: &CodexOperationSelection<'_>,
        evidence: &CodexOperationEvidence,
    ) -> Result<OperationPayload, String> {
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
        match operation.native_kind {
            "McpToolCall" => item.mcp_detail(),
            "FileChange" => item.edit_detail(),
            "CommandExecution" => item.command_detail(),
            _ => None,
        }
        .ok_or_else(|| unavailable.to_owned())
    }

    /// Verifies the exact Claude MCP tool_use occurrence and returns only its
    /// bounded, screened input under the independent Claude MCP grant.
    fn claude_mcp_payload(
        &self,
        operation: &OperationDefinition,
        selection: &CodexOperationSelection<'_>,
        evidence: &CodexOperationEvidence,
    ) -> Result<OperationPayload, String> {
        let unavailable = operation.unavailable;
        let Header::Claude(session_id) = &self.header else {
            return Err(unavailable.into());
        };
        if session_id != selection.session_id {
            return Err(unavailable.into());
        }
        let member = selection
            .event_id
            .strip_prefix("claude:")
            .and_then(|event| event.strip_prefix(&self.cursor.generation))
            .and_then(|event| event.strip_prefix(':'))
            .ok_or(unavailable)?;
        let (native, tail) = member
            .split_once(operation.event_suffix)
            .ok_or(unavailable)?;
        let index: usize = tail
            .parse()
            .ok()
            .filter(|index| *index < MAX_CLAUDE_BLOCKS)
            .ok_or(unavailable)?;
        let offset = usize::try_from(selection.revision).map_err(|_| unavailable)?;
        let proofs = self.admitted_offsets(std::iter::once(evidence.admitted_cursor.as_str()));
        let proof_end = proofs
            .get(evidence.admitted_cursor.as_str())
            .ok_or(unavailable)?;
        let rows = self.rows(std::iter::once(offset.to_string()))?;
        let line = match rows.get(&offset.to_string()) {
            Some(RowMatch::Exact { line, end }) if *end <= *proof_end => *line,
            _ => return Err(unavailable.into()),
        };
        let row: ClaudeOperationRow = serde_json::from_slice(line).map_err(|_| unavailable)?;
        let native_uuid = match row.uuid.as_deref().filter(|uuid| !uuid.is_empty()) {
            Some(uuid) => hash(uuid.as_bytes())[..32].to_owned(),
            None => format!("offset-{}", selection.revision),
        };
        if row.kind != "assistant"
            || row.session_id != selection.session_id
            || row.worker_id.is_some()
            || row.is_meta
            || native_uuid != native
            || observed_timestamp(&row.timestamp) != Some(evidence.observed_at_unix_ns)
        {
            return Err(unavailable.into());
        }
        let block = row
            .message
            .content
            .get(index)
            .ok_or_else(|| unavailable.to_owned())?;
        let call_id = block
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .ok_or(unavailable)?;
        let name = block
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or(unavailable)?;
        if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_use")
            || hash(call_id.as_bytes())[..32] != *selection.operation_id
            || agenturm_kernel::mcp_tool_labels(name).is_none()
        {
            return Err(unavailable.into());
        }
        let input = block.get("input").ok_or(unavailable)?;
        let encoded = serde_json::to_string(input).map_err(|_| unavailable)?;
        if text_preview(UserContent::Text(encoded.clone()))
            .ok()
            .as_deref()
            != Some(&encoded)
        {
            return Err(unavailable.into());
        }
        let mut fields = serde_json::Map::new();
        fields.insert("arguments".into(), input.clone());
        Ok(OperationPayload::Mcp(fields))
    }

    /// Verifies the exact Claude assistant row carrying the selected file-edit
    /// tool_use and extracts a bounded edit payload. The row is located by its
    /// committed byte offset (the observation revision), and the event and
    /// operation identities must match the row's own bytes. The turn linkage
    /// is proven store-side by the selected occurrence; this row itself never
    /// repeats it natively, so nothing about the turn is re-derived here.
    fn claude_edit_payload(
        &self,
        operation: &OperationDefinition,
        selection: &CodexOperationSelection<'_>,
        evidence: &CodexOperationEvidence,
    ) -> Result<OperationPayload, String> {
        let unavailable = operation.unavailable;
        let Header::Claude(session_id) = &self.header else {
            return Err(unavailable.into());
        };
        if session_id != selection.session_id {
            return Err(unavailable.into());
        }
        let member = selection
            .event_id
            .strip_prefix("claude:")
            .and_then(|event| event.strip_prefix(&self.cursor.generation))
            .and_then(|event| event.strip_prefix(':'))
            .ok_or(unavailable)?;
        let (native, tail) = member.split_once(":file-edit-").ok_or(unavailable)?;
        let index: usize = tail
            .parse()
            .ok()
            .filter(|index| *index < MAX_CLAUDE_BLOCKS)
            .ok_or(unavailable)?;
        let offset = usize::try_from(selection.revision).map_err(|_| unavailable)?;
        let proofs = self.admitted_offsets(std::iter::once(evidence.admitted_cursor.as_str()));
        let proof_end = proofs
            .get(evidence.admitted_cursor.as_str())
            .ok_or(unavailable)?;
        let rows = self.rows(std::iter::once(offset.to_string()))?;
        let line = match rows.get(&offset.to_string()) {
            Some(RowMatch::Exact { line, end }) if *end <= *proof_end => *line,
            _ => return Err(unavailable.into()),
        };
        let row: ClaudeOperationRow = serde_json::from_slice(line).map_err(|_| unavailable)?;
        let native_uuid = match row.uuid.as_deref().filter(|uuid| !uuid.is_empty()) {
            Some(uuid) => hash(uuid.as_bytes())[..32].to_owned(),
            None => format!("offset-{}", selection.revision),
        };
        if row.kind != "assistant"
            || row.session_id != selection.session_id
            || row.worker_id.is_some()
            || row.is_meta
            || native_uuid != native
            || observed_timestamp(&row.timestamp) != Some(evidence.observed_at_unix_ns)
        {
            return Err(unavailable.into());
        }
        let block = row
            .message
            .content
            .get(index)
            .ok_or_else(|| unavailable.to_owned())?;
        let operation_id = block
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .ok_or(unavailable)?;
        let name = block
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or(unavailable)?;
        if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_use")
            || hash(operation_id.as_bytes())[..32] != *selection.operation_id
        {
            return Err(unavailable.into());
        }
        let detail = claude_edit_detail(name, block.get("input")).ok_or(unavailable)?;
        Ok(OperationPayload::ClaudeEdit(detail))
    }

    /// Verifies one exact Pi assistant toolCall behind a retained file-change
    /// fact, then extracts only a bounded, grant-protected edit preview.
    fn pi_edit_payload(
        &self,
        operation: &OperationDefinition,
        selection: &CodexOperationSelection<'_>,
        evidence: &CodexOperationEvidence,
    ) -> Result<OperationPayload, String> {
        let unavailable = operation.unavailable;
        let Header::Pi(header) = &self.header else {
            return Err(unavailable.into());
        };
        if header.session_id.as_deref().unwrap_or(&header.id) != selection.session_id
            || header
                .agent_id
                .as_deref()
                .is_some_and(|worker| !worker.is_empty())
            || selection.revision != 0
        {
            return Err(unavailable.into());
        }
        let member = selection
            .event_id
            .strip_prefix("pi:")
            .and_then(|event| event.strip_prefix(&self.cursor.generation))
            .and_then(|event| event.strip_prefix(':'))
            .ok_or(unavailable)?;
        let (native_row, tail) = member
            .rsplit_once(operation.event_suffix)
            .ok_or(unavailable)?;
        let index: usize = tail
            .parse()
            .ok()
            .filter(|index| *index < MAX_CLAUDE_BLOCKS)
            .ok_or(unavailable)?;
        if !valid_id(native_row) {
            return Err(unavailable.into());
        }
        let proofs = self.admitted_offsets(std::iter::once(evidence.admitted_cursor.as_str()));
        let proof_end = proofs
            .get(evidence.admitted_cursor.as_str())
            .ok_or(unavailable)?;
        let rows = self.rows(std::iter::once(native_row.to_owned()))?;
        let line = match rows.get(native_row) {
            Some(RowMatch::Exact { line, end }) if end <= proof_end => *line,
            _ => return Err(unavailable.into()),
        };
        let row: serde_json::Value = serde_json::from_slice(line).map_err(|_| unavailable)?;
        if row.get("type").and_then(serde_json::Value::as_str) != Some("message")
            || row.get("id").and_then(serde_json::Value::as_str) != Some(native_row)
            || row["message"]["role"].as_str() != Some("assistant")
            || observed_timestamp(
                row.get("timestamp")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            ) != Some(evidence.observed_at_unix_ns)
        {
            return Err(unavailable.into());
        }
        let block = row["message"]["content"]
            .as_array()
            .and_then(|blocks| blocks.get(index))
            .ok_or(unavailable)?;
        if block.get("type").and_then(serde_json::Value::as_str) != Some("toolCall")
            || pi_operation_id(
                block
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(unavailable)?,
            )
            .as_deref()
                != Some(selection.operation_id)
        {
            return Err(unavailable.into());
        }
        let name = block
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or(unavailable)?;
        let arguments = block.get("arguments").ok_or(unavailable)?;
        let Some(summary) = agenturm_kernel::pi_file_change(name, Some(arguments)) else {
            return Err(unavailable.into());
        };
        if Some(summary.digest.as_str()) != evidence.digest.as_deref()
            || Some(summary.files) != evidence.files
        {
            return Err(unavailable.into());
        }
        pi_edit_detail(name, arguments)
            .map(OperationPayload::PiEdit)
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
    thread_source: Option<String>,
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
    command: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_payload_value")]
    status: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_payload_value")]
    exit_code: Option<serde_json::Value>,
}

impl CodexOperationItem {
    fn command_detail(self) -> Option<OperationPayload> {
        let recorded_invocation: RecordedInvocation = serde_json::from_value(self.command?).ok()?;
        if let RecordedInvocation::Argv(args) = &recorded_invocation
            && (!(1..=64).contains(&args.len()) || args[0].trim().is_empty())
        {
            return None;
        }
        let text = recorded_invocation.searchable_text();
        if text.trim().is_empty()
            || text.chars().any(char::is_control)
            || command_credential_flags(&text)
            || bounded_text_preview(text.to_string(), MAX_TEXT_BYTES).ok()? != text
        {
            return None;
        }
        let confirmed_failure = command_failure::confirmed_failure(
            &recorded_invocation,
            self.status.as_ref(),
            self.exit_code.as_ref(),
            self.result.as_ref(),
        );
        Some(OperationPayload::Command {
            recorded_invocation,
            confirmed_failure,
        })
    }

    fn mcp_detail(self) -> Option<OperationPayload> {
        let fields = [("arguments", self.arguments), ("result", self.result)]
            .into_iter()
            .filter_map(|(field, value)| value.map(|value| (field.to_owned(), value)))
            .collect::<serde_json::Map<_, _>>();
        let text = serde_json::to_string(&fields).ok()?;
        if fields.is_empty() || text_preview(UserContent::Text(text.clone())).ok()? != text {
            return None;
        }
        Some(OperationPayload::Mcp(fields))
    }

    fn edit_detail(self) -> Option<OperationPayload> {
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
        Some(OperationPayload::Edit { changes })
    }
}

fn command_credential_flags(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let arguments: Vec<_> = lower
        .split_ascii_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect();
    arguments.iter().any(|argument| {
        let name = argument.split_once('=').map_or(*argument, |(name, _)| name);
        matches!(
            name,
            "--password"
                | "--passwd"
                | "--pass"
                | "--auth"
                | "--authorization"
                | "--token"
                | "--access-token"
                | "--oauth2-bearer"
                | "--secret"
                | "--client-secret"
                | "--api-key"
        )
    }) || arguments
        .windows(2)
        .any(|pair| matches!(pair[0], "-u" | "--user") && pair[1].contains(':'))
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

#[derive(Deserialize, Serialize)]
pub struct EditChange {
    pub path: String,
    #[serde(flatten)]
    edit: NativeFileChange,
}

#[derive(Serialize)]
#[serde(untagged)]
enum OperationPayload {
    Mcp(serde_json::Map<String, serde_json::Value>),
    Edit {
        changes: Vec<EditChange>,
    },
    Command {
        recorded_invocation: RecordedInvocation,
        #[serde(skip)]
        confirmed_failure: bool,
    },
    ClaudeEdit(ClaudeEditDetail),
    OpenCodeEdit(OpenCodeEditDetail),
    PiEdit(PiEditDetail),
}

impl OperationPayload {
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
            Self::Edit { changes } => display_edit_changes(changes),
            Self::Command {
                recorded_invocation,
                ..
            } => serde_json::to_string(recorded_invocation).map_err(|_| ())?,
            Self::ClaudeEdit(edit) => edit.display(),
            Self::OpenCodeEdit(edit) => edit.display(),
            Self::PiEdit(edit) => edit.display(),
        };
        serde_json::to_string(&display).map_err(|_| ())
    }
}

/// One verified Claude Code file-edit tool_use, bounded for disclosure.
/// Every revealed string is a truncated prefix of at most
/// `MAX_EDIT_TEXT_BYTES` with an explicit marker; credential-shaped or
/// otherwise unsafe text withholds the entire payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeEditDetail {
    tool: String,
    file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edits: Option<Vec<ClaudeEditPair>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClaudeEditPair {
    old_string: String,
    new_string: String,
}

impl ClaudeEditDetail {
    fn display(&self) -> String {
        let action = match self.tool.as_str() {
            "Write" | "write" => "Wrote",
            "Edit" | "edit" => "Edited",
            _ => "Multi-edited",
        };
        let mut text = format!("{action} {}", self.file_path);
        if let Some(content) = &self.content {
            text.push('\n');
            text.push_str(content);
        }
        if let (Some(old), Some(new)) = (&self.old_string, &self.new_string) {
            text.push('\n');
            text.push_str(old);
            text.push_str("\n→\n");
            text.push_str(new);
        }
        for pair in self.edits.as_deref().unwrap_or_default() {
            text.push('\n');
            text.push_str(&pair.old_string);
            text.push_str("\n→\n");
            text.push_str(&pair.new_string);
        }
        if self.truncated {
            text.push_str("\n(edited detail was truncated)");
        }
        text
    }
}

/// One verified native OpenCode patch, bounded for disclosure. Each revealed
/// path is a truncated prefix of at most 512 bytes with an
/// explicit marker; credential-shaped or otherwise unsafe text withholds the
/// entire payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenCodeEditDetail {
    files: Vec<String>,
    reported_files: u16,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

/// One bounded Pi `write` or `edit` toolCall. The exact native call is
/// displayed as a proposal; its tool result is not treated as proof that the
/// filesystem change completed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PiEditDetail {
    tool: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(rename = "oldText", skip_serializing_if = "Option::is_none")]
    old_text: Option<String>,
    #[serde(rename = "newText", skip_serializing_if = "Option::is_none")]
    new_text: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

impl PiEditDetail {
    fn display(&self) -> String {
        let mut text = format!("Pi {} target {}", self.tool, self.path);
        if let Some(content) = &self.content {
            text.push('\n');
            text.push_str(content);
        }
        if let (Some(old), Some(new)) = (&self.old_text, &self.new_text) {
            text.push('\n');
            text.push_str(old);
            text.push_str("\n→\n");
            text.push_str(new);
        }
        if self.truncated {
            text.push_str("\n(edit preview was truncated)");
        }
        text
    }
}

impl OpenCodeEditDetail {
    fn display(&self) -> String {
        let mut text = format!(
            "Recorded {} changed paths (diffs and add/delete counts unavailable)",
            self.reported_files
        );
        for path in &self.files {
            text.push('\n');
            text.push_str(path);
        }
        if self.truncated {
            text.push_str("\n(file list was truncated)");
        }
        text
    }
}

/// Extracts one bounded edit payload from a verified Claude tool_use input.
/// Recognition is shared with the adapter through the protocol summary
/// helper, so detail is only revealed for a tool_use whose edit counts were
/// already reported. Any unsupported or unsafe shape yields `None`.
fn claude_edit_detail(name: &str, input: Option<&serde_json::Value>) -> Option<ClaudeEditDetail> {
    let fields = input?.as_object()?;
    agenturm_kernel::claude_file_edit_summary(name, input)?;
    let file_path = fields
        .get("file_path")
        .and_then(serde_json::Value::as_str)?;
    if !edit_path(file_path) {
        return None;
    }
    let mut detail = ClaudeEditDetail {
        tool: name.to_owned(),
        file_path: file_path.to_owned(),
        content: None,
        old_string: None,
        new_string: None,
        edits: None,
        truncated: false,
    };
    match name.to_ascii_lowercase().as_str() {
        "write" => {
            let content = bounded_edit_text(fields.get("content")?.as_str()?)?;
            detail.truncated = content.1;
            detail.content = Some(content.0);
        }
        "edit" => {
            let old = bounded_edit_text(fields.get("old_string")?.as_str()?)?;
            let new = bounded_edit_text(fields.get("new_string")?.as_str()?)?;
            detail.truncated = old.1 || new.1;
            detail.old_string = Some(old.0);
            detail.new_string = Some(new.0);
        }
        "multiedit" => {
            let edits = fields.get("edits")?.as_array()?;
            let mut pairs = Vec::new();
            let mut bytes = 0;
            for edit in edits {
                let old = bounded_edit_text(edit.get("old_string")?.as_str()?)?;
                let new = bounded_edit_text(edit.get("new_string")?.as_str()?)?;
                if bytes + old.0.len() + new.0.len() > MAX_ROW {
                    detail.truncated = true;
                    break;
                }
                bytes += old.0.len() + new.0.len();
                detail.truncated |= old.1 || new.1;
                pairs.push(ClaudeEditPair {
                    old_string: old.0,
                    new_string: new.0,
                });
            }
            if pairs.is_empty() {
                return None;
            }
            detail.edits = Some(pairs);
        }
        _ => return None,
    }
    Some(detail)
}

fn pi_edit_detail(name: &str, input: &serde_json::Value) -> Option<PiEditDetail> {
    let fields = input.as_object()?;
    let path = fields.get("path")?.as_str()?;
    if !edit_path(path) {
        return None;
    }
    let tool = name.to_ascii_lowercase();
    let mut detail = PiEditDetail {
        tool: tool.clone(),
        path: path.to_owned(),
        content: None,
        old_text: None,
        new_text: None,
        truncated: false,
    };
    match tool.as_str() {
        "write" => {
            let content = bounded_edit_text(fields.get("content")?.as_str()?)?;
            detail.truncated = content.1;
            detail.content = Some(content.0);
        }
        "edit" => {
            let old = bounded_edit_text(fields.get("oldText")?.as_str()?)?;
            let new = bounded_edit_text(fields.get("newText")?.as_str()?)?;
            detail.truncated = old.1 || new.1;
            detail.old_text = Some(old.0);
            detail.new_text = Some(new.0);
        }
        _ => return None,
    }
    Some(detail)
}

fn pi_operation_id(value: &str) -> Option<String> {
    if valid_id(value) {
        return Some(value.to_owned());
    }
    let (call, item) = value.split_once('|')?;
    let call_suffix = call.strip_prefix("call_")?;
    let item_suffix = item
        .strip_prefix("fc_")
        .or_else(|| item.strip_prefix("ctc_"))?;
    if call_suffix.is_empty() || item_suffix.is_empty() || !valid_id(call) || !valid_id(item) {
        return None;
    }
    Some(format!("pi-call/{}", hash(value.as_bytes())))
}

/// Truncates to the edit text bound on a character boundary and applies the
/// shared preview safety filter. The flag reports whether text was cut.
fn bounded_edit_text(text: &str) -> Option<(String, bool)> {
    let (prefix, truncated) = if text.len() <= MAX_EDIT_TEXT_BYTES {
        (text, false)
    } else {
        let mut end = MAX_EDIT_TEXT_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        (&text[..end], true)
    };
    if prefix.is_empty() {
        return None;
    }
    let filtered = bounded_text_preview(prefix.to_owned(), MAX_EDIT_TEXT_BYTES).ok()?;
    Some((filtered, truncated))
}

fn display_edit_changes(changes: &[EditChange]) -> String {
    changes
        .iter()
        .map(EditChange::display)
        .collect::<Vec<_>>()
        .join("\n\n")
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

/// One Claude assistant row under edit-detail verification. Native Claude
/// rows carry many extra fields, so like `ClaudeUserRow` this reads only the
/// bound fields and ignores the rest; every extracted value is re-checked
/// against the selection before disclosure.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOperationRow {
    #[serde(rename = "type")]
    kind: String,
    uuid: Option<String>,
    session_id: String,
    #[serde(rename = "agentId")]
    worker_id: Option<String>,
    #[serde(default)]
    is_meta: bool,
    timestamp: String,
    message: ClaudeOperationMessage,
}

#[derive(Deserialize)]
struct ClaudeOperationMessage {
    content: Vec<serde_json::Value>,
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

fn valid_operation_id(id: &str) -> bool {
    if valid_id(id) {
        return true;
    }
    id.strip_prefix("pi-call/").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
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

#[cfg(test)]
mod codex_guardian_tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn exact_open_inode_and_checkpoint_cannot_authorize_guardian_content() {
        static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);
        for (payload, permitted) in [
            (r#"{"id":"same-thread","source":"cli"}"#, true),
            (
                r#"{"id":"same-thread","thread_source":"guardian_review","source":"cli"}"#,
                false,
            ),
            (
                r#"{"id":"same-thread","source":{"subagent":{"other":"guardian"}}}"#,
                false,
            ),
            (
                r#"{"id":"same-thread","thread_source":"guardian_review","thread_source":"cli"}"#,
                false,
            ),
        ] {
            let header = format!("{{\"type\":\"session_meta\",\"payload\":{payload}}}\n");
            let bytes =
                format!("{header}{{\"type\":\"event_msg\",\"payload\":\"private fixture\"}}\n");
            let path = std::env::temp_dir().join(format!(
                "agenturm-guardian-proof-{}-{}.jsonl",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed),
            ));
            let mut writer = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            writer.write_all(bytes.as_bytes()).unwrap();
            drop(writer);
            let file = open_member(&path, Harness::Codex).unwrap();
            let stamp = FileStamp::from_file(&file).unwrap();
            let header_digest = hash(header.as_bytes());
            let generation =
                hash(format!("{}:{}:{header_digest}", stamp.device, stamp.inode).as_bytes());
            let cursor = format!(
                "v2:{}:{}:{}:{}:{header_digest}:{generation}:turn",
                stamp.device,
                stamp.inode,
                bytes.len(),
                hash(bytes.as_bytes())
            );
            let mut source = BoundSource {
                file,
                stamp,
                scope: ContentGrantScope::new(
                    ExtensionId::parse("harness.codex.fixture").unwrap(),
                    ContentKind::CodexUserText,
                    [1; 32],
                ),
                harness: Harness::Codex,
                source_path: path.clone(),
                operation_kind: None,
                opencode_stamp: None,
            };
            let result = source.verified_prefix(&cursor);
            std::fs::remove_file(path).unwrap();
            if permitted {
                assert!(
                    matches!(result.unwrap().header, Header::Codex(session) if session == "same-thread")
                );
            } else {
                assert!(matches!(result, Err(error) if error == SOURCE_CHANGED));
            }
        }
    }
}

#[cfg(test)]
mod cf11_cf12_operation_proof_tests {
    use super::*;
    use serde_json::{Value, json};

    const TIME: &str = "2026-11-20T12:00:00Z";

    fn make_prefix(header: Value, rows: &[Value], harness: Harness) -> (VerifiedPrefix, String) {
        let mut bytes = serde_json::to_vec(&header).unwrap();
        bytes.push(b'\n');
        let header_len = bytes.len();
        for row in rows {
            bytes.extend(serde_json::to_vec(row).unwrap());
            bytes.push(b'\n');
        }
        let header_digest = hash(&bytes[..header_len]);
        let generation = hash(format!("7:11:{header_digest}").as_bytes());
        let cursor = format!(
            "v2:7:11:{}:{}:{header_digest}:{generation}:turn",
            bytes.len(),
            hash(&bytes)
        );
        let proof = cursor_proof(&cursor).unwrap().unwrap();
        let parsed_header = match harness {
            Harness::Pi => Header::Pi(serde_json::from_slice(&bytes[..header_len]).unwrap()),
            Harness::Claude => Header::Claude(
                serde_json::from_slice::<ClaudeHeader>(&bytes[..header_len])
                    .unwrap()
                    .session_id,
            ),
            _ => unreachable!(),
        };
        (
            VerifiedPrefix {
                bytes,
                cursor: proof,
                header: parsed_header,
                header_len,
            },
            cursor,
        )
    }

    fn evidence(cursor: &str, timestamp: &str) -> CodexOperationEvidence {
        CodexOperationEvidence {
            cursor: cursor.to_owned(),
            admitted_cursor: cursor.to_owned(),
            turn_id: "turn-1".into(),
            observed_at_unix_ns: observed_timestamp(timestamp).unwrap(),
            digest: None,
            files: None,
        }
    }

    fn stale_receipt(cursor: &str) -> String {
        let mut fields = cursor.split(':').map(str::to_owned).collect::<Vec<_>>();
        fields[4] = "0".repeat(64);
        fields.join(":")
    }

    fn pi_selection(generation: &str, operation_id: &str) -> CodexOperationSelection<'static> {
        CodexOperationSelection {
            session_id: "pi-session",
            event_id: Box::leak(
                format!("pi:{generation}:assistant-row:file-change-0").into_boxed_str(),
            ),
            revision: 0,
            operation_id: Box::leak(operation_id.to_owned().into_boxed_str()),
            turn_id: Some("turn-1"),
        }
    }

    #[test]
    fn pi_edit_reveal_requires_exact_admitted_assistant_tool_and_summary() {
        let args = json!({"path":"src/main.rs","oldText":"before","newText":"after"});
        let row = json!({"type":"message","id":"assistant-row","timestamp":TIME,
        "message":{"role":"assistant","content":[
            {"type":"toolCall","id":"call_1|fc_1","name":"edit","arguments":args}
        ]}});
        let (prefix, cursor) = make_prefix(
            json!({"type":"session","id":"pi-session"}),
            &[row.clone()],
            Harness::Pi,
        );
        let summary = agenturm_kernel::pi_file_change("edit", Some(&args)).unwrap();
        let operation = operation_definition(Harness::Pi, OperationDetailKind::Edit).unwrap();
        let mut proof = evidence(&cursor, TIME);
        proof.digest = Some(summary.digest.clone());
        proof.files = Some(summary.files);
        // Pi's compound native call/item identity is committed as a namespaced
        // digest by both the adapter and private-content verifier.
        let operation_id = pi_operation_id("call_1|fc_1").unwrap();
        let selection = pi_selection(&prefix.cursor.generation, &operation_id);
        let payload = prefix
            .operation_payload(OperationDetailKind::Edit, &operation, &selection, &proof)
            .unwrap();
        assert!(matches!(payload, OperationPayload::PiEdit(_)));

        let wrong_operation = pi_selection(&prefix.cursor.generation, "other-call");
        assert!(
            prefix
                .operation_payload(
                    OperationDetailKind::Edit,
                    &operation,
                    &wrong_operation,
                    &proof
                )
                .is_err()
        );
        let wrong_index = CodexOperationSelection {
            event_id: Box::leak(
                format!(
                    "pi:{}:assistant-row:file-change-1",
                    prefix.cursor.generation
                )
                .into_boxed_str(),
            ),
            ..selection
        };
        assert!(
            prefix
                .operation_payload(OperationDetailKind::Edit, &operation, &wrong_index, &proof)
                .is_err()
        );

        let mut changed_summary = proof.clone();
        changed_summary.digest = Some("0".repeat(64));
        assert!(
            prefix
                .operation_payload(
                    OperationDetailKind::Edit,
                    &operation,
                    &selection,
                    &changed_summary
                )
                .is_err()
        );

        let mismatched_session = CodexOperationSelection {
            session_id: "another-session",
            ..selection
        };
        assert!(
            prefix
                .operation_payload(
                    OperationDetailKind::Edit,
                    &operation,
                    &mismatched_session,
                    &proof
                )
                .is_err()
        );
    }

    #[test]
    fn pi_edit_reveal_withholds_unsafe_inputs_and_stale_receipts() {
        let unsafe_args = json!({"path":"src/main.rs","oldText":"before",
            "newText":"token=ghp_123456789012345678901234567890"});
        let row = json!({"type":"message","id":"assistant-row","timestamp":TIME,
        "message":{"role":"assistant","content":[
            {"type":"toolCall","id":"call_1|fc_1","name":"edit","arguments":unsafe_args}
        ]}});
        let (unsafe_prefix, cursor) = make_prefix(
            json!({"type":"session","id":"pi-session"}),
            &[row],
            Harness::Pi,
        );
        let summary = agenturm_kernel::pi_file_change("edit", Some(&unsafe_args)).unwrap();
        let operation = operation_definition(Harness::Pi, OperationDetailKind::Edit).unwrap();
        let mut proof = evidence(&cursor, TIME);
        proof.digest = Some(summary.digest);
        proof.files = Some(summary.files);
        let operation_id = pi_operation_id("call_1|fc_1").unwrap();
        let selection = pi_selection(&unsafe_prefix.cursor.generation, &operation_id);
        assert!(
            unsafe_prefix
                .operation_payload(OperationDetailKind::Edit, &operation, &selection, &proof)
                .is_err()
        );

        let mut stale = proof.clone();
        stale.admitted_cursor = stale_receipt(&stale.admitted_cursor);
        assert!(
            unsafe_prefix
                .operation_payload(OperationDetailKind::Edit, &operation, &selection, &stale)
                .is_err()
        );
    }

    fn claude_selection(
        generation: &str,
        uuid: &str,
        offset: u32,
        call_id: &str,
    ) -> CodexOperationSelection<'static> {
        let native = hash(uuid.as_bytes())[..32].to_owned();
        CodexOperationSelection {
            session_id: "claude-session",
            event_id: Box::leak(format!("claude:{generation}:{native}:mcp-use-1").into_boxed_str()),
            revision: offset,
            operation_id: Box::leak(hash(call_id.as_bytes())[..32].to_owned().into_boxed_str()),
            turn_id: Some("turn-1"),
        }
    }

    #[test]
    fn claude_mcp_reveal_extracts_only_the_selected_verified_tool_use() {
        let call_id = "toolu_123";
        let row = json!({"type":"assistant","sessionId":"claude-session","uuid":"assistant-uuid",
        "timestamp":TIME,"message":{"content":[
            {"type":"text","text":"unrelated"},
            {"type":"tool_use","id":call_id,"name":"mcp__server__lookup",
             "input":{"query":"exact input"}}
        ]}});
        let header = json!({"sessionId":"claude-session"});
        let header_bytes = serde_json::to_vec(&header).unwrap();
        let offset = (header_bytes.len() + 1) as u32;
        let (prefix, cursor) = make_prefix(header, &[row], Harness::Claude);
        let selection =
            claude_selection(&prefix.cursor.generation, "assistant-uuid", offset, call_id);
        let operation = operation_definition(Harness::Claude, OperationDetailKind::Mcp).unwrap();
        let proof = evidence(&cursor, TIME);
        let payload = prefix
            .operation_payload(OperationDetailKind::Mcp, &operation, &selection, &proof)
            .unwrap();
        let OperationPayload::Mcp(fields) = payload else {
            panic!("expected MCP payload");
        };
        assert_eq!(
            fields.get("arguments"),
            Some(&json!({"query":"exact input"}))
        );
        assert_eq!(operation.grant, ContentKind::ClaudeMcpPayload);
        assert_ne!(operation.grant, ContentKind::CodexMcpPayload);

        let wrong_call = claude_selection(
            &prefix.cursor.generation,
            "assistant-uuid",
            offset,
            "different-call",
        );
        assert!(
            prefix
                .operation_payload(OperationDetailKind::Mcp, &operation, &wrong_call, &proof)
                .is_err()
        );
        let wrong_index = CodexOperationSelection {
            event_id: Box::leak(
                format!("{}", selection.event_id.replace("mcp-use-1", "mcp-use-0"))
                    .into_boxed_str(),
            ),
            ..selection
        };
        assert!(
            prefix
                .operation_payload(OperationDetailKind::Mcp, &operation, &wrong_index, &proof)
                .is_err()
        );
    }

    #[test]
    fn claude_mcp_reveal_rejects_wrong_native_role_name_and_mutated_receipt() {
        let call_id = "toolu_123";
        let row = json!({"type":"assistant","sessionId":"claude-session","uuid":"assistant-uuid",
        "timestamp":TIME,"message":{"content":[
            {"type":"text","text":"unrelated"},
            {"type":"tool_use","id":call_id,"name":"mcp__server__lookup",
             "input":{"query":"exact input"}}
        ]}});
        let header = json!({"sessionId":"claude-session"});
        let header_bytes = serde_json::to_vec(&header).unwrap();
        let offset = (header_bytes.len() + 1) as u32;
        let (prefix, cursor) = make_prefix(header, &[row.clone()], Harness::Claude);
        let selection =
            claude_selection(&prefix.cursor.generation, "assistant-uuid", offset, call_id);
        let operation = operation_definition(Harness::Claude, OperationDetailKind::Mcp).unwrap();
        let proof = evidence(&cursor, TIME);
        let mut stale = proof.clone();
        stale.admitted_cursor = stale_receipt(&stale.admitted_cursor);
        assert!(
            prefix
                .operation_payload(OperationDetailKind::Mcp, &operation, &selection, &stale)
                .is_err()
        );

        let worker_row = json!({"type":"assistant","sessionId":"claude-session","agentId":"worker",
        "uuid":"assistant-uuid","timestamp":TIME,"message":{"content":[
            {"type":"text","text":"unrelated"},
            {"type":"tool_use","id":call_id,"name":"mcp__server__lookup",
             "input":{"query":"exact input"}}
        ]}});
        let (worker_prefix, worker_cursor) = make_prefix(
            json!({"sessionId":"claude-session"}),
            &[worker_row],
            Harness::Claude,
        );
        let worker_selection = claude_selection(
            &worker_prefix.cursor.generation,
            "assistant-uuid",
            offset,
            call_id,
        );
        assert!(
            worker_prefix
                .operation_payload(
                    OperationDetailKind::Mcp,
                    &operation,
                    &worker_selection,
                    &evidence(&worker_cursor, TIME)
                )
                .is_err()
        );

        let non_mcp_row = json!({"type":"assistant","sessionId":"claude-session","uuid":"assistant-uuid",
        "timestamp":TIME,"message":{"content":[
            {"type":"text","text":"unrelated"},
            {"type":"tool_use","id":call_id,"name":"Bash",
             "input":{"command":"secret"}}
        ]}});
        let (non_mcp_prefix, non_mcp_cursor) = make_prefix(
            json!({"sessionId":"claude-session"}),
            &[non_mcp_row],
            Harness::Claude,
        );
        assert!(
            non_mcp_prefix
                .operation_payload(
                    OperationDetailKind::Mcp,
                    &operation,
                    &claude_selection(
                        &non_mcp_prefix.cursor.generation,
                        "assistant-uuid",
                        offset,
                        call_id,
                    ),
                    &evidence(&non_mcp_cursor, TIME)
                )
                .is_err()
        );
    }
}
