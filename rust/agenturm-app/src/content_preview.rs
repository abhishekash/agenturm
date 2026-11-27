//! Host verification for explicit private previews from installed harnesses.

use agenturm_kernel::{ExtensionId, ReadSnapshot};
use agenturm_store::Store;
use agenturm_ui::{OperationDetailKind, ReviewEditSelection};
#[cfg(not(target_os = "linux"))]
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "Usage: agenturm content binding|grant|revoke --kind pi.mcp_payload|codex.mcp_payload|claude.mcp_payload|...\n       agenturm content preview --kind pi.mcp_payload|codex.mcp_payload|claude.mcp_payload --db <path> --config <path> --source-id <id> --session-id <id> --turn-id <id> --operation-id <id> --event-id <id> --revision <revision>";

/// Proof-eligible metadata for a single retained user prompt. The selection
/// grants no authority and is checked anew before any text is delivered.
#[cfg(target_os = "linux")]
pub use agenturm_content::PromptReference;
#[cfg(not(target_os = "linux"))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PromptReference {
    pub event_id: String,
    pub revision: u32,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
    pub snapshot_id: Option<ReadSnapshot>,
    pub history_identity: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentOptions {
    action: Action,
    db: PathBuf,
    source_id: ExtensionId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentOperationKind {
    Mcp,
    PiMcp,
    ClaudeMcp,
    Edit,
    PiEdit,
    ClaudeEdit,
    OpenCodeEdit,
    Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Binding {
        config: PathBuf,
        kind: Option<ContentOperationKind>,
    },
    Grant {
        config: PathBuf,
        kind: Option<ContentOperationKind>,
    },
    Revoke {
        kind: Option<ContentOperationKind>,
    },
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
    PreviewOperation {
        config: PathBuf,
        kind: ContentOperationKind,
        session_id: String,
        turn_id: Option<String>,
        operation_id: String,
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
    let db = if action == "binding" {
        if values.contains_key("--db") {
            return Err(USAGE.into());
        }
        PathBuf::new()
    } else {
        PathBuf::from(values.remove("--db").ok_or(USAGE)?)
    };
    let source_id = ExtensionId::parse(values.remove("--source-id").ok_or(USAGE)?)
        .map_err(|_| "invalid content source ID")?;
    let kind = match values.remove("--kind") {
        None => None,
        Some("codex.mcp_payload") => Some(ContentOperationKind::Mcp),
        Some("pi.mcp_payload") => Some(ContentOperationKind::PiMcp),
        Some("claude.mcp_payload") => Some(ContentOperationKind::ClaudeMcp),
        Some("codex.edit_detail") => Some(ContentOperationKind::Edit),
        Some("pi.edit_detail") => Some(ContentOperationKind::PiEdit),
        Some("claude.edit_detail") => Some(ContentOperationKind::ClaudeEdit),
        Some("opencode.edit_detail") => Some(ContentOperationKind::OpenCodeEdit),
        Some("codex.command_detail") => Some(ContentOperationKind::Command),
        _ => return Err(USAGE.into()),
    };
    let action = match action.as_str() {
        "binding" => Action::Binding {
            config: PathBuf::from(values.remove("--config").ok_or(USAGE)?),
            kind,
        },
        "grant" => Action::Grant {
            config: PathBuf::from(values.remove("--config").ok_or(USAGE)?),
            kind,
        },
        "revoke" => Action::Revoke { kind },
        "list" if kind.is_none() => Action::List {
            config: PathBuf::from(values.remove("--config").ok_or(USAGE)?),
            session_id: values.remove("--session-id").ok_or(USAGE)?.to_owned(),
        },
        "preview" => {
            let config = PathBuf::from(values.remove("--config").ok_or(USAGE)?);
            let event_id = values.remove("--event-id").ok_or(USAGE)?;
            if event_id.len() > 256 {
                return Err("Content preview event ID exceeds its limit".into());
            }
            let revision = values
                .remove("--revision")
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or("invalid content revision")?;
            if let Some(kind) = kind {
                let turn_id = values.remove("--turn-id");
                if !matches!(
                    kind,
                    ContentOperationKind::Mcp
                        | ContentOperationKind::PiMcp
                        | ContentOperationKind::ClaudeMcp
                ) && turn_id.is_none()
                {
                    return Err(USAGE.into());
                }
                Action::PreviewOperation {
                    config,
                    kind,
                    session_id: values.remove("--session-id").ok_or(USAGE)?.to_owned(),
                    turn_id: turn_id.map(str::to_owned),
                    operation_id: values.remove("--operation-id").ok_or(USAGE)?.to_owned(),
                    event_id: event_id.to_owned(),
                    revision,
                }
            } else {
                Action::Preview {
                    config,
                    session_id: values.remove("--session-id").map(str::to_owned),
                    event_id: event_id.to_owned(),
                    revision,
                }
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
    worker_id: Option<&str>,
    writer: Option<&Store>,
) -> Result<Vec<PromptReference>, String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(writer) = writer {
            linux::list(writer, config, source_id, session_id, worker_id)
        } else {
            linux::list(
                &linux::open_store(db)?,
                config,
                source_id,
                session_id,
                worker_id,
            )
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (db, config, source_id, session_id, worker_id, writer);
        Err("Prompt preview is available on Linux".into())
    }
}

/// Sends an exact user-selected prompt to its final sink while the proof
/// and authorization fence is held. The sink must complete delivery on flush.
pub(super) struct PromptSelection<'a> {
    pub session_id: &'a str,
    pub worker_id: Option<&'a str>,
    pub event_id: &'a str,
    pub revision: u32,
    pub snapshot_id: Option<ReadSnapshot>,
    pub history_identity: Option<&'a str>,
}

#[cfg(target_os = "linux")]
pub(super) fn open_source_authority<'a>(
    db: &Path,
    config: &'a Path,
) -> Result<(Store, Box<agenturm_content::SourceResolver<'a>>), String> {
    Ok((linux::open_store(db)?, Box::new(linux::resolver(config))))
}

#[cfg(target_os = "linux")]
pub(super) fn source_resolver(
    config: &Path,
) -> impl Fn(&ExtensionId) -> Result<agenturm_content::ResolvedSource, String> + '_ {
    linux::resolver(config)
}

#[cfg(target_os = "linux")]
pub(super) fn with_verified_prompt<T>(
    db: &Path,
    config: &Path,
    source_id: &ExtensionId,
    selection: PromptSelection<'_>,
    consume: impl FnOnce(&agenturm_content::VerifiedPrompt) -> T,
) -> Result<T, String> {
    let store = linux::open_store(db)?;
    agenturm_content::with_selected_prompt(
        &store,
        &linux::resolver(config),
        source_id,
        agenturm_content::PromptSelection {
            session_id: Some(selection.session_id),
            worker_id: selection.worker_id,
            event_id: selection.event_id,
            revision: selection.revision,
            snapshot_id: selection.snapshot_id,
            history_identity: selection.history_identity,
        },
        consume,
    )
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
    {
        let owned;
        let writer = if let Some(writer) = writer {
            writer
        } else {
            owned = linux::open_store(db)?;
            &owned
        };
        agenturm_content::reveal_selected_prompt(
            writer,
            &linux::resolver(config),
            source_id,
            agenturm_content::PromptSelection {
                session_id: Some(selection.session_id),
                worker_id: selection.worker_id,
                event_id: selection.event_id,
                revision: selection.revision,
                snapshot_id: selection.snapshot_id,
                history_identity: selection.history_identity,
            },
            output,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (db, config, source_id, selection, output, writer);
        Err("Prompt preview is available on Linux".into())
    }
}

pub(super) fn reveal_operation_detail(
    db: &Path,
    config: &Path,
    source_id: &ExtensionId,
    kind: OperationDetailKind,
    selection: &agenturm_store::CodexOperationSelection<'_>,
    output: &mut dyn Write,
    writer: Option<&Store>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(writer) = writer {
            linux::reveal_operation_fenced(writer, config, source_id, kind, selection, output, true)
        } else {
            linux::reveal_operation_fenced(
                &linux::open_store(db)?,
                config,
                source_id,
                kind,
                selection,
                output,
                true,
            )
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (db, config, source_id, kind, selection, output, writer);
        Err("Operation detail preview is available on Linux".into())
    }
}

pub(super) fn reveal_review_edit(
    db: &Path,
    config: &Path,
    selected: &ReviewEditSelection,
    output: &mut dyn Write,
    writer: Option<&Store>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let operation = &selected.operation;
        if operation.kind != OperationDetailKind::Edit {
            return Err("The selected Review occurrence is not a file edit".into());
        }
        let source = ExtensionId::parse(&operation.source_id).map_err(|error| error.to_string())?;
        let opened;
        let store = if let Some(writer) = writer {
            writer
        } else {
            opened = linux::open_store(db)?;
            &opened
        };
        agenturm_content::with_verified_edit(
            store,
            &linux::resolver(config),
            &source,
            &agenturm_store::CodexOperationSelection {
                session_id: &operation.session_id,
                turn_id: Some(&operation.turn_id),
                operation_id: &operation.operation_id,
                event_id: &operation.event_id,
                revision: operation.revision,
            },
            |edit| {
                if store
                    .history_identity()
                    .map_err(|error| error.to_string())?
                    != selected.history_identity
                    || !store
                        .contains_observation_at_snapshot(
                            &source,
                            &operation.event_id,
                            operation.revision,
                            selected.snapshot_id,
                        )
                        .map_err(|error| error.to_string())?
                {
                    return Err("The edit is unavailable at the selected Review snapshot".into());
                }
                store
                    .verify_content_writer()
                    .map_err(|error| error.to_string())?;
                edit.write_terminal(output)
            },
        )?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (db, config, selected, output, writer);
        Err("Review edit preview is available on Linux".into())
    }
}

pub fn execute_content_stdout(options: &ContentOptions) -> Result<(), String> {
    if matches!(
        options.action,
        Action::Preview { .. } | Action::PreviewOperation { .. }
    ) {
        crate::with_private_stdout(|output| execute_content(options, output))
    } else {
        execute_content(options, &mut std::io::stdout().lock())
    }
}

/// The caller's sink must complete all private writes and flush promptly
/// or return an error. The CLI uses a deadline-aware stdout sink.
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
    use agenturm_store::{CodexOperationSelection, ContentKind};
    pub(super) fn execute(options: &ContentOptions, output: &mut dyn Write) -> Result<(), String> {
        match &options.action {
            Action::Binding { config, kind } => {
                let source = agenturm_content::bind(
                    &resolver(config),
                    &options.source_id,
                    kind.map(content_kind),
                )?;
                verify_kind_harness(kind.as_ref(), &source)?;
                let binding = agenturm_storage_service::GrantBinding::from(source.scope());
                let mut bytes = serde_json::to_vec(&binding).map_err(|error| error.to_string())?;
                bytes.push(b'\n');
                write_output(output, &bytes)
            }
            Action::Grant { config, kind } => {
                let mut store = open_store(&options.db)?;
                let source = agenturm_content::bind(
                    &resolver(config),
                    &options.source_id,
                    kind.map(content_kind),
                )?;
                verify_kind_harness(kind.as_ref(), &source)?;
                store
                    .grant_content(source.scope())
                    .map_err(|error| error.to_string())?;
                let description = kind.map_or("user-text", |kind| operation_description(kind));
                write_output(
                    output,
                    format!(
                        "Approved {} {description} preview for this source.\n",
                        source.label(),
                    )
                    .as_bytes(),
                )
            }
            Action::Revoke { kind: Some(kind) } => {
                let mut store = open_store(&options.db)?;
                let grant = match kind {
                    ContentOperationKind::Mcp => ContentKind::CodexMcpPayload,
                    ContentOperationKind::PiMcp => ContentKind::PiMcpDetail,
                    ContentOperationKind::ClaudeMcp => ContentKind::ClaudeMcpPayload,
                    ContentOperationKind::Edit => ContentKind::CodexEditDetail,
                    ContentOperationKind::PiEdit => ContentKind::PiEditDetail,
                    ContentOperationKind::ClaudeEdit => ContentKind::ClaudeEditDetail,
                    ContentOperationKind::OpenCodeEdit => ContentKind::OpenCodeEditDetail,
                    ContentOperationKind::Command => ContentKind::CodexCommandDetail,
                };
                store
                    .revoke_content(&options.source_id, grant)
                    .map_err(|error| error.to_string())?;
                write_output(
                    output,
                    format!(
                        "{} {} preview approval is revoked.\n",
                        revoke_label(*kind),
                        operation_description(*kind)
                    )
                    .as_bytes(),
                )
            }
            Action::Revoke { kind: None } => {
                let mut store = open_store(&options.db)?;
                store
                    .revoke_source_content(&options.source_id)
                    .map_err(|error| error.to_string())?;
                write_output(output, b"Content preview approval is revoked.\n")
            }
            Action::List { config, session_id } => {
                let store = open_store(&options.db)?;
                let references = list(&store, config, &options.source_id, session_id, None)?;
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
                &open_store(&options.db)?,
                config,
                &options.source_id,
                session_id.as_deref(),
                event_id,
                *revision,
                output,
            ),
            Action::PreviewOperation {
                config,
                kind,
                session_id,
                turn_id,
                operation_id,
                event_id,
                revision,
            } => agenturm_content::reveal_operation_fenced(
                &open_store(&options.db)?,
                &resolver(config),
                &options.source_id,
                content_kind(*kind),
                &CodexOperationSelection {
                    session_id,
                    turn_id: turn_id.as_deref(),
                    operation_id,
                    event_id,
                    revision: *revision,
                },
                output,
                false,
            ),
        }
    }

    pub(super) fn open_store(db: &Path) -> Result<Store, String> {
        if !db.is_file() {
            return Err("Content commands require an existing observation database".into());
        }
        Store::open_collector(db).map_err(|error| error.to_string())
    }

    fn shared_kind(kind: OperationDetailKind) -> agenturm_content::OperationDetailKind {
        match kind {
            OperationDetailKind::Mcp => agenturm_content::OperationDetailKind::Mcp,
            OperationDetailKind::Edit => agenturm_content::OperationDetailKind::Edit,
        }
    }
    fn content_kind(kind: ContentOperationKind) -> agenturm_content::OperationDetailKind {
        match kind {
            ContentOperationKind::Mcp => agenturm_content::OperationDetailKind::Mcp,
            ContentOperationKind::PiMcp => agenturm_content::OperationDetailKind::PiMcp,
            ContentOperationKind::ClaudeMcp => agenturm_content::OperationDetailKind::Mcp,
            // The edit detail kind is shared across harnesses: the pinned
            // source's plugin decides which provider contract applies.
            ContentOperationKind::Edit
            | ContentOperationKind::PiEdit
            | ContentOperationKind::ClaudeEdit
            | ContentOperationKind::OpenCodeEdit => agenturm_content::OperationDetailKind::Edit,
            ContentOperationKind::Command => agenturm_content::OperationDetailKind::Command,
        }
    }

    /// The named CLI kind must resolve to the same durable grant kind the
    /// pinned harness actually defines, so one provider's flag can never
    /// silently approve another provider's private detail.
    fn verify_kind_harness(
        kind: Option<&ContentOperationKind>,
        source: &agenturm_content::BoundSource,
    ) -> Result<(), String> {
        let Some(kind) = kind else {
            return Ok(());
        };
        let expected = match kind {
            ContentOperationKind::Mcp => ContentKind::CodexMcpPayload,
            ContentOperationKind::PiMcp => ContentKind::PiMcpDetail,
            ContentOperationKind::ClaudeMcp => ContentKind::ClaudeMcpPayload,
            ContentOperationKind::Edit => ContentKind::CodexEditDetail,
            ContentOperationKind::PiEdit => ContentKind::PiEditDetail,
            ContentOperationKind::ClaudeEdit => ContentKind::ClaudeEditDetail,
            ContentOperationKind::OpenCodeEdit => ContentKind::OpenCodeEditDetail,
            ContentOperationKind::Command => ContentKind::CodexCommandDetail,
        };
        if source.scope().kind() != expected {
            return Err(format!(
                "{} preview requires its matching pinned harness source",
                operation_description(*kind)
            ));
        }
        Ok(())
    }

    fn operation_description(kind: ContentOperationKind) -> &'static str {
        match kind {
            ContentOperationKind::Mcp => "MCP payload",
            ContentOperationKind::PiMcp => "MCP payload",
            ContentOperationKind::ClaudeMcp => "MCP payload",
            ContentOperationKind::Edit
            | ContentOperationKind::PiEdit
            | ContentOperationKind::ClaudeEdit
            | ContentOperationKind::OpenCodeEdit => "file edit",
            ContentOperationKind::Command => "command invocation",
        }
    }

    fn revoke_label(kind: ContentOperationKind) -> &'static str {
        match kind {
            ContentOperationKind::ClaudeMcp => "Claude Code",
            ContentOperationKind::PiMcp => "Pi",
            ContentOperationKind::Mcp
            | ContentOperationKind::Edit
            | ContentOperationKind::Command => "Codex",
            ContentOperationKind::ClaudeEdit => "Claude Code",
            ContentOperationKind::PiEdit => "Pi",
            ContentOperationKind::OpenCodeEdit => "OpenCode",
        }
    }
    pub(super) fn resolver(
        config: &Path,
    ) -> impl Fn(&ExtensionId) -> Result<agenturm_content::ResolvedSource, String> + '_ {
        move |id| {
            let config = crate::load_collection_config(config)?;
            config.content_source(id)
        }
    }
    pub(super) fn list(
        store: &Store,
        config: &Path,
        source: &ExtensionId,
        session: &str,
        worker: Option<&str>,
    ) -> Result<Vec<PromptReference>, String> {
        agenturm_content::list_for_worker(store, &resolver(config), source, session, worker)
    }
    pub(super) fn reveal_fenced(
        store: &Store,
        config: &Path,
        source: &ExtensionId,
        session: Option<&str>,
        event: &str,
        revision: u32,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        agenturm_content::reveal_fenced(
            store,
            &resolver(config),
            source,
            session,
            event,
            revision,
            output,
        )
    }
    pub(super) fn reveal_operation_fenced(
        store: &Store,
        config: &Path,
        source: &ExtensionId,
        kind: OperationDetailKind,
        selection: &CodexOperationSelection<'_>,
        output: &mut dyn Write,
        terminal: bool,
    ) -> Result<(), String> {
        let provider_kind = if kind == OperationDetailKind::Mcp
            && (selection.event_id.starts_with("pi:")
                || source.as_str() == "harness.pi"
                || source.as_str().starts_with("harness.pi."))
        {
            agenturm_content::OperationDetailKind::PiMcp
        } else {
            shared_kind(kind)
        };
        agenturm_content::reveal_operation_fenced(
            store,
            &resolver(config),
            source,
            provider_kind,
            selection,
            output,
            terminal,
        )
    }
    fn write_output(output: &mut dyn Write, bytes: &[u8]) -> Result<(), String> {
        output
            .write_all(bytes)
            .and_then(|()| output.flush())
            .map_err(|_| "cannot write content output".into())
    }
}
