//! Bounded investigation over native, grant-verified source records.

use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::{ExtensionId, ObservationPosition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

mod failures;

pub(super) const USAGE: &str = concat!(
    "agenturm investigate <instruction|path|command> <query> --source-id <id> --session-id <id> ",
    "[--turn-id <id> [--worker-id <id>]] [--limit 1..100] [--cursor <token>] --db <path> --config <json>\n",
    "Use --failed with command to require a source-confirmed failure.\n",
    "agenturm investigate failures [--source-id <id>] [--session-id <id>] ",
    "[--kind lifecycle|tool|mcp|check|trace|log] [--limit 1..100] [--cursor <token>] --db <path>\n",
    "agenturm investigate source <event-id> --reference <token> --db <path> --config <json>\n",
    "Python receivers use --receiver-protocol python-v0.2.8 --endpoint <loopback-http-url> --token-file <path>.\n",
    "Python searches accept --service, --pages 1..5, --cursor and --format text|json.\n",
    "For broker history use agenturm broker investigate with --broker-config <json> in place of --db and --config."
);

const MAX_QUERY_BYTES: usize = 4_096;
const MAX_TOKEN_BYTES: usize = 4_096;
const MAX_ID_BYTES: usize = 256;
const MAX_PAGE: usize = 100;
#[cfg(target_os = "linux")]
const COCKPIT_PAGE_LIMIT: usize = 30;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvestigationOptions {
    db: Option<PathBuf>,
    config: Option<PathBuf>,
    action: Action,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Failures(failures::FailureSearch),
    Search(Search),
    Source(SourceReference),
    #[cfg(target_os = "linux")]
    Legacy(Box<crate::remote_history::legacy_investigation::Options>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Search {
    kind: EvidenceKind,
    query: String,
    scope: Scope,
    limit: usize,
    failed: bool,
    cursor: Option<SearchCursor>,
    snapshot_id: Option<ReadSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    source_id: ExtensionId,
    session_id: String,
    turn_id: Option<String>,
    worker_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceKind {
    Instruction,
    Path,
    Command,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchCursor {
    version: u16,
    search_sha256: String,
    before: ObservationPosition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceReference {
    version: u16,
    history_sha256: String,
    scope: Scope,
    kind: EvidenceKind,
    position: ObservationPosition,
    operation_id: Option<String>,
    grant_sha256: String,
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn token<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, String> {
    if raw.len() > MAX_TOKEN_BYTES {
        return Err("Investigation token exceeds its size limit".into());
    }
    serde_json::from_str(raw).map_err(|_| "Invalid investigation token".into())
}

fn required<'a>(flags: &mut BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, String> {
    flags
        .remove(name)
        .ok_or_else(|| format!("Supply {name}\n{USAGE}"))
}

fn validate_query(kind: EvidenceKind, query: &str) -> Result<(), String> {
    if query.trim().is_empty()
        || query.len() > MAX_QUERY_BYTES
        || query.chars().any(char::is_control)
        || (kind != EvidenceKind::Path && !query.chars().any(char::is_alphanumeric))
        || (kind == EvidenceKind::Path && query.len() > 512)
    {
        return Err("Supply a bounded, single-line investigation query".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn pinned_snapshot(
    selected: Option<ReadSnapshot>,
    before: Option<&ObservationPosition>,
) -> Result<Option<ReadSnapshot>, String> {
    if let Some(cursor) = before {
        if selected.is_some_and(|snapshot| snapshot != cursor.snapshot_id) {
            return Err("Investigation cursor differs from the selected history snapshot".into());
        }
        return Ok(Some(cursor.snapshot_id));
    }
    Ok(selected)
}

pub(super) fn parse(args: &[String], broker: bool) -> Result<InvestigationOptions, String> {
    if args.iter().skip(2).any(|arg| {
        matches!(
            arg.as_str(),
            "--receiver-protocol" | "--endpoint" | "--token-file"
        )
    }) {
        if broker {
            return Err(
                "Python investigation uses an explicit receiver, without broker history flags"
                    .into(),
            );
        }
        #[cfg(target_os = "linux")]
        return crate::remote_history::legacy_investigation::parse(args).map(|options| {
            InvestigationOptions {
                db: None,
                config: None,
                action: Action::Legacy(Box::new(options)),
            }
        });
        #[cfg(not(target_os = "linux"))]
        return Err("Python receiver investigation requires Linux".into());
    }
    if let [verb, rest @ ..] = args
        && verb == "failures"
    {
        return failures::parse(rest, broker);
    }
    let [verb, query, flags @ ..] = args else {
        return Err(USAGE.into());
    };
    let mut options = BTreeMap::new();
    let mut flags = flags.iter();
    let mut failed = false;
    while let Some(flag) = flags.next() {
        if flag == "--failed" {
            if verb != "command" || failed {
                return Err(USAGE.into());
            }
            failed = true;
            continue;
        }
        let value = flags.next().ok_or(USAGE)?;
        if !flag.starts_with("--")
            || value.is_empty()
            || options.insert(flag.as_str(), value.as_str()).is_some()
        {
            return Err(USAGE.into());
        }
    }
    let (db, config) = if broker {
        (None, None)
    } else {
        (
            Some(required(&mut options, "--db")?.into()),
            Some(required(&mut options, "--config")?.into()),
        )
    };

    let action = if verb == "source" {
        let reference: SourceReference = token(required(&mut options, "--reference")?)?;
        if reference.position.event_id != *query || !reference.valid() {
            return Err("Source reference differs from the selected occurrence".into());
        }
        Action::Source(reference)
    } else {
        let kind = match verb.as_str() {
            "instruction" => EvidenceKind::Instruction,
            "path" => EvidenceKind::Path,
            "command" => EvidenceKind::Command,
            _ => return Err(USAGE.into()),
        };
        validate_query(kind, query)?;
        let source_id = ExtensionId::parse(required(&mut options, "--source-id")?)
            .map_err(|_| "Invalid investigation source")?;
        let session_id = required(&mut options, "--session-id")?;
        let turn_id = options.remove("--turn-id");
        let worker_id = options.remove("--worker-id");
        if !id(session_id)
            || turn_id.is_some_and(|v| !id(v))
            || worker_id.is_some_and(|v| !id(v) || turn_id.is_none())
        {
            return Err("Invalid investigation session, turn, or worker scope".into());
        }
        let limit = options
            .remove("--limit")
            .map(str::parse::<usize>)
            .transpose()
            .map_err(|_| "Investigation limit must be 1..100")?
            .unwrap_or(30);
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err("Investigation limit must be 1..100".into());
        }
        let cursor: Option<SearchCursor> = options.remove("--cursor").map(token).transpose()?;
        if cursor.as_ref().is_some_and(|cursor| {
            cursor.version != 1
                || !digest(&cursor.search_sha256)
                || !id(&cursor.before.event_id)
                || cursor.before.harness_id != source_id.as_str()
        }) {
            return Err("Invalid investigation cursor".into());
        }
        Action::Search(Search {
            kind,
            query: query.clone(),
            scope: Scope {
                source_id,
                session_id: session_id.into(),
                turn_id: turn_id.map(str::to_owned),
                worker_id: worker_id.map(str::to_owned),
            },
            limit,
            failed,
            cursor,
            snapshot_id: None,
        })
    };
    if !options.is_empty() {
        return Err(USAGE.into());
    }
    Ok(InvestigationOptions { db, config, action })
}

impl SourceReference {
    fn valid(&self) -> bool {
        let version_valid = match self.version {
            1 => digest(&self.grant_sha256),
            2 | 3 => {
                self.kind == EvidenceKind::Command
                    && (if self.version == 2 {
                        self.grant_sha256.is_empty()
                    } else {
                        digest(&self.grant_sha256)
                    })
                    && self.position.event_id.starts_with("codex:")
                    && self.position.event_id.ends_with(":tool")
            }
            _ => false,
        };
        version_valid
            && id(&self.position.event_id)
            && self.position.harness_id == self.scope.source_id.as_str()
            && id(&self.scope.session_id)
            && self.scope.turn_id.as_deref().is_some_and(id)
            && self.scope.worker_id.as_deref().is_none_or(id)
            && digest(&self.history_sha256)
            && match self.kind {
                EvidenceKind::Instruction => self.operation_id.is_none(),
                EvidenceKind::Path | EvidenceKind::Command => {
                    self.scope.worker_id.is_none() && self.operation_id.as_deref().is_some_and(id)
                }
            }
    }
}

/// Writes matching evidence while its grant is fenced. A custom output sink
/// must finish writes and flushes within the disclosure deadline.
pub fn execute_investigation(
    options: &InvestigationOptions,
    output: impl Write,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if let Action::Legacy(options) = &options.action {
            return crate::remote_history::legacy_investigation::execute(options, output);
        }
        linux::execute_local(options, output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (options, output);
        Err("Native source investigations require Linux".into())
    }
}

#[cfg(target_os = "linux")]
pub(super) use linux::{cockpit, execute_broker};

#[cfg(target_os = "linux")]
mod linux {
    pub(crate) mod cockpit;

    use super::*;
    use agenturm_content::{SourceResolver, VerifiedCommand, VerifiedEdit, VerifiedPrompt};
    use agenturm_kernel::{
        Observation, ObservationKind, ObservationPage, ObservationQuery, ObservationScope,
        ObservationStore,
    };
    use agenturm_storage_service::{
        NativeClientError, NativeStore, PrivateEvidenceFailure, PrivateEvidenceKind,
        PrivateEvidenceSelection,
    };
    use agenturm_store::{CodexOperationSelection, Store, WorkspaceLedger};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use unicode_segmentation::UnicodeSegmentation;

    const MAX_PROOFS: usize = 12;
    const WITHHELD: &str =
        "Current approved source evidence is unavailable for the selected occurrence";

    enum Authority<'a> {
        Local(&'a Store, &'a SourceResolver<'a>),
        Broker(&'a NativeStore),
    }

    enum Proven<'a> {
        Prompt(&'a VerifiedPrompt),
        Edit(&'a VerifiedEdit),
        Command(&'a VerifiedCommand),
    }

    enum ProofError {
        Unavailable,
        Failed(String),
    }

    impl From<NativeClientError> for ProofError {
        fn from(error: NativeClientError) -> Self {
            match error {
                NativeClientError::Private(PrivateEvidenceFailure::Unavailable) => {
                    Self::Unavailable
                }
                _ => Self::Failed(error.to_string()),
            }
        }
    }

    impl Proven<'_> {
        fn matches(&self, query: &str, phrase: &[String]) -> bool {
            let text = match self {
                Self::Prompt(prompt) => std::borrow::Cow::Borrowed(prompt.text.as_str()),
                Self::Edit(edit) => return edit.contains_path(query),
                Self::Command(command) => command.recorded_invocation.searchable_text(),
            };
            words(&text)
                .windows(phrase.len())
                .any(|window| window == phrase)
        }

        fn grant(&self) -> String {
            let fingerprint = match self {
                Self::Prompt(prompt) => prompt.grant_binding,
                Self::Edit(edit) => edit.grant_binding,
                Self::Command(command) => command.grant_binding,
            };
            fingerprint
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }

        fn confirms(&self, reference: &SourceReference) -> bool {
            let scope = &reference.scope;
            let timestamp = reference.position.observed_at_unix_ns;
            match self {
                Self::Prompt(prompt) => {
                    prompt.session_id == scope.session_id
                        && prompt.worker_id == scope.worker_id
                        && Some(prompt.turn_id.as_str()) == scope.turn_id.as_deref()
                        && prompt.observed_at_unix_ns == timestamp
                }
                Self::Edit(edit) => {
                    scope.worker_id.is_none()
                        && Some(edit.turn_id.as_str()) == scope.turn_id.as_deref()
                        && edit.observed_at_unix_ns == timestamp
                }
                Self::Command(command) => {
                    scope.worker_id.is_none()
                        && Some(command.turn_id.as_str()) == scope.turn_id.as_deref()
                        && command.observed_at_unix_ns == timestamp
                }
            }
        }
    }

    impl Authority<'_> {
        fn history(&self) -> &dyn ObservationStore {
            match self {
                Self::Local(store, _) => *store,
                Self::Broker(store) => *store,
            }
        }

        fn prove<R>(
            &self,
            selected: &SourceReference,
            failed: bool,
            consume: impl FnOnce(Proven<'_>) -> Result<R, String>,
        ) -> Result<R, ProofError> {
            let scope = &selected.scope;
            let position = &selected.position;
            let turn = scope.turn_id.as_deref().ok_or(ProofError::Unavailable)?;
            let source = &scope.source_id;
            let session = &scope.session_id;
            let event = &position.event_id;
            let revision = position.revision;
            match selected.kind {
                EvidenceKind::Instruction => match self {
                    Self::Local(store, resolver) => agenturm_content::with_verified_prompt(
                        store,
                        *resolver,
                        source,
                        Some(session),
                        event,
                        revision,
                        |prompt| {
                            with_retained_occurrence(store, selected, || {
                                consume(Proven::Prompt(prompt))
                            })
                        },
                    )
                    .map_err(|_| ProofError::Unavailable)?,
                    Self::Broker(store) => store
                        .with_verified_prompt_at_snapshot(
                            source,
                            session,
                            event,
                            revision,
                            Some(position.snapshot_id),
                            |prompt| consume(Proven::Prompt(prompt)),
                        )
                        .map_err(Into::into),
                },
                EvidenceKind::Path | EvidenceKind::Command => {
                    let operation_id = selected
                        .operation_id
                        .as_deref()
                        .ok_or(ProofError::Unavailable)?;
                    let local = CodexOperationSelection {
                        event_id: event,
                        revision,
                        session_id: session,
                        turn_id: Some(turn),
                        operation_id,
                    };
                    let private = |kind| PrivateEvidenceSelection {
                        source_id: source.clone(),
                        session_id: session.clone(),
                        event_id: event.clone(),
                        revision,
                        kind,
                    };
                    let snapshot = position.snapshot_id;
                    match (self, selected.kind) {
                        (Self::Local(store, resolver), EvidenceKind::Path) => {
                            agenturm_content::with_verified_edit(
                                store,
                                *resolver,
                                source,
                                &local,
                                |edit| {
                                    with_retained_occurrence(store, selected, || {
                                        consume(Proven::Edit(edit))
                                    })
                                },
                            )
                            .map_err(|_| ProofError::Unavailable)?
                        }
                        (Self::Local(store, resolver), EvidenceKind::Command) => {
                            let verify = if failed {
                                agenturm_content::with_verified_failed_command
                            } else {
                                agenturm_content::with_verified_command
                            };
                            verify(
                                store,
                                *resolver,
                                source,
                                &local,
                                |command: &VerifiedCommand| {
                                    with_retained_occurrence(store, selected, || {
                                        consume(Proven::Command(command))
                                    })
                                },
                            )
                            .map_err(|_| ProofError::Unavailable)?
                        }
                        (Self::Broker(store), EvidenceKind::Path) => {
                            let selection = private(PrivateEvidenceKind::CodexEditDetail {
                                turn_id: turn.into(),
                                operation_id: operation_id.into(),
                            });
                            store
                                .with_verified_edit(&selection, snapshot, |edit| {
                                    consume(Proven::Edit(edit))
                                })
                                .map_err(Into::into)
                        }
                        (Self::Broker(store), EvidenceKind::Command) => {
                            let selection = private(if failed {
                                PrivateEvidenceKind::CodexFailedCommandDetail {
                                    turn_id: turn.into(),
                                    operation_id: operation_id.into(),
                                }
                            } else {
                                PrivateEvidenceKind::CodexCommandDetail {
                                    turn_id: turn.into(),
                                    operation_id: operation_id.into(),
                                }
                            });
                            store
                                .with_verified_command(&selection, snapshot, |command| {
                                    consume(Proven::Command(command))
                                })
                                .map_err(Into::into)
                        }
                        (_, EvidenceKind::Instruction) => Err(ProofError::Unavailable),
                    }
                }
            }
        }
    }

    fn with_retained_occurrence<T>(
        store: &Store,
        selected: &SourceReference,
        consume: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, ProofError> {
        let position = &selected.position;
        let retained = store
            .contains_observation_at_snapshot(
                &selected.scope.source_id,
                &position.event_id,
                position.revision,
                position.snapshot_id,
            )
            .map_err(|error| ProofError::Failed(error.to_string()))?;
        if !retained {
            return Err(ProofError::Unavailable);
        }
        store.verify_content_writer().map_err(|_| {
            ProofError::Failed("Investigation history changed during source proof".into())
        })?;
        consume().map_err(ProofError::Failed)
    }

    pub(super) fn execute_local(
        options: &InvestigationOptions,
        mut output: impl Write,
    ) -> Result<(), String> {
        let db = options.db.as_deref().ok_or(USAGE)?;
        if let Action::Failures(search) = &options.action {
            let store = Store::open_read_only(db).map_err(|error| error.to_string())?;
            return failures::emit_page(search, &store, &local_history(db)?, &mut output);
        }
        let config = options.config.as_deref().ok_or(USAGE)?;
        let (store, resolver) = crate::content_preview::open_source_authority(db, config)?;
        execute(
            &options.action,
            Authority::Local(&store, resolver.as_ref()),
            &local_history(db)?,
            output,
        )
    }

    fn local_history(db: &Path) -> Result<String, String> {
        let binding = WorkspaceLedger::open(db)
            .and_then(|ledger| ledger.task_draft_binding())
            .map_err(|error| error.to_string())?;
        hash(("agenturm-investigation-local/1", binding))
    }

    pub(crate) fn execute_broker(
        options: &InvestigationOptions,
        store: NativeStore,
        endpoint: (&Path, &str, u32, u32),
        output: impl Write,
    ) -> Result<(), String> {
        let identity = store
            .history_identity()
            .map_err(|error| error.to_string())?;
        let history = hash(("agenturm-investigation-broker/1", endpoint, &identity))?;
        let store = store
            .with_workspace_identity(&identity)
            .map_err(|error| error.to_string())?;
        execute(&options.action, Authority::Broker(&store), &history, output)
    }

    fn execute(
        action: &Action,
        authority: Authority<'_>,
        history: &str,
        mut output: impl Write,
    ) -> Result<(), String> {
        match action {
            Action::Failures(search) => {
                failures::emit_page(search, authority.history(), history, &mut output)
            }
            Action::Search(search) => search_page(search, &authority, history, &mut output),
            Action::Source(reference) => source(reference, &authority, history, &mut output),
            Action::Legacy(_) => Err("Python investigation needs an explicit receiver".into()),
        }
    }

    fn page(
        authority: &Authority<'_>,
        scope: &Scope,
        before: Option<ObservationPosition>,
        selected: Option<ReadSnapshot>,
        limit: usize,
    ) -> Result<ObservationPage, String> {
        let snapshot_id = pinned_snapshot(selected, before.as_ref())?;
        let query = ObservationQuery {
            harness_id: Some(scope.source_id.as_str().into()),
            session_id: Some(scope.session_id.clone()),
            before,
            snapshot_id,
            limit,
        };
        let history = authority.history();
        match scope.turn_id.as_deref() {
            Some(turn) => {
                history.query_worker_turn_observations(&query, turn, scope.worker_id.as_deref())
            }
            None => history.query_observations(&query),
        }
    }

    fn words(text: &str) -> Vec<String> {
        text.unicode_words().map(str::to_lowercase).collect()
    }

    struct SearchCoverage {
        snapshot_id: ReadSnapshot,
        examined: usize,
        matched: usize,
        verified: usize,
        unavailable: usize,
        next: Option<String>,
    }

    fn scan_search(
        search: &Search,
        authority: &Authority<'_>,
        history: &str,
        mut visit: impl FnMut(&Observation, &SourceReference, Proven<'_>) -> Result<(), String>,
    ) -> Result<SearchCoverage, String> {
        let identity = if search.failed {
            hash((2, history, search.kind, &search.scope, &search.query))?
        } else {
            hash((1, history, search.kind, &search.scope, &search.query))?
        };
        if let Some(cursor) = &search.cursor
            && cursor.search_sha256 != hash((&identity, &cursor.before))?
        {
            return Err(
                "Investigation cursor belongs to another query, history, or snapshot".into(),
            );
        }
        let before = search.cursor.as_ref().map(|cursor| cursor.before.clone());
        let page = page(
            authority,
            &search.scope,
            before,
            search.snapshot_id,
            search.limit,
        )?;
        let phrase = words(&search.query);
        if search.kind != EvidenceKind::Path && phrase.is_empty() {
            return Err("Phrase queries require a word or number".into());
        }
        let mut examined = 0;
        let mut attempted = 0;
        let mut unavailable = 0;
        let mut matched = 0;
        let mut last = None;
        for row in page.observations.iter().rev() {
            let target = if !search.failed || failures::reported_tool_failure(row) {
                candidate(row, search.kind, history, page.snapshot_id)
            } else {
                None
            };
            if target.is_some() && attempted == MAX_PROOFS {
                break;
            }
            examined += 1;
            last = Some(position(row, page.snapshot_id));
            let Some(target) = target else { continue };
            attempted += 1;
            match authority.prove(&target, search.failed, |proof| {
                if !proof.confirms(&target) {
                    return Ok(None);
                }
                if !proof.matches(&search.query, &phrase) {
                    return Ok(Some(false));
                }
                visit(row, &target, proof)?;
                Ok(Some(true))
            }) {
                Ok(Some(true)) => matched += 1,
                Ok(Some(false)) => {}
                Ok(None) | Err(ProofError::Unavailable) => unavailable += 1,
                Err(ProofError::Failed(error)) => return Err(error),
            }
        }
        let more = page.has_more || examined < page.observations.len();
        let next = if more {
            let before = last.ok_or("Investigation pagination did not advance")?;
            let search_sha256 = hash((&identity, &before))?;
            Some(encoded(&SearchCursor {
                version: 1,
                search_sha256,
                before,
            })?)
        } else {
            None
        };
        Ok(SearchCoverage {
            snapshot_id: page.snapshot_id,
            examined,
            matched,
            verified: attempted - unavailable,
            unavailable,
            next,
        })
    }

    fn search_page(
        search: &Search,
        authority: &Authority<'_>,
        history: &str,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        let coverage = scan_search(search, authority, history, |row, target, proof| {
            let bound = bind_verified_reference(target, &proof, search.failed);
            let reference = encoded(&bound)?;
            let mut record = json!({
                "type": "match", "kind": target.kind, "event_id": row.event_id, "scope": target.scope,
                "revision": row.revision, "observed_at_unix_ns": row.observed_at_unix_ns,
                "source_reference": reference
            });
            if search.failed {
                record["confirmed_failure"] = json!(true);
            }
            match proof {
                Proven::Prompt(prompt) => record["excerpt"] = json!(prompt.text),
                Proven::Edit(_) => record["excerpt"] = json!(search.query),
                Proven::Command(command) => {
                    record["recorded_invocation"] = json!(command.recorded_invocation)
                }
            };
            emit(output, record)?;
            Ok(())
        })?;
        emit(
            output,
            json!({
                "type": "coverage", "kind": search.kind, "format": "agenturm-investigation/1",
                "snapshot_id": coverage.snapshot_id, "observations_examined": coverage.examined,
                "matched": coverage.matched, "verified_candidates": coverage.verified,
                "unavailable_candidates": coverage.unavailable, "next_cursor": coverage.next,
                "scope_exhausted": coverage.next.is_none(), "retained_native_evidence_only": true,
            }),
        )
    }

    fn bind_verified_reference(
        target: &SourceReference,
        proof: &Proven<'_>,
        failed: bool,
    ) -> SourceReference {
        SourceReference {
            version: if failed { 3 } else { 1 },
            grant_sha256: proof.grant(),
            ..target.clone()
        }
    }

    fn verify_reference(
        reference: &SourceReference,
        authority: &Authority<'_>,
        history: &str,
        consume: impl FnOnce(Proven<'_>, bool) -> Result<(), String>,
    ) -> Result<(), String> {
        if reference.history_sha256 != history {
            return Err("Source reference belongs to another history".into());
        }
        // The shared proof binds the exact native occurrence, and its fenced
        // snapshot check requires that row at the original receipt watermark.
        let failed = matches!(reference.version, 2 | 3);
        let disclosed = authority.prove(reference, failed, |proof| {
            if !proof.confirms(reference)
                || (reference.version != 2 && proof.grant() != reference.grant_sha256)
            {
                return Ok(false);
            }
            consume(proof, failed)?;
            Ok(true)
        });
        match disclosed {
            Ok(true) => Ok(()),
            Ok(false) | Err(ProofError::Unavailable) => Err(WITHHELD.into()),
            Err(ProofError::Failed(error)) => Err(error),
        }
    }

    fn source(
        reference: &SourceReference,
        authority: &Authority<'_>,
        history: &str,
        output: &mut dyn Write,
    ) -> Result<(), String> {
        verify_reference(reference, authority, history, |proof, failed| {
            let evidence = match proof {
                Proven::Prompt(prompt) => json!({"text": prompt.text}),
                Proven::Edit(edit) => match &edit.provider_detail {
                    Some(detail) => json!({"provider_detail": detail}),
                    None => json!({"changes": edit.changes}),
                },
                Proven::Command(command) => {
                    json!({"recorded_invocation": command.recorded_invocation})
                }
            };
            let mut record = json!({
                "type": "source", "kind": reference.kind, "scope": reference.scope,
                "event_id": reference.position.event_id, "revision": reference.position.revision,
                "observed_at_unix_ns": reference.position.observed_at_unix_ns,
                "evidence": evidence
            });
            if failed {
                record["confirmed_failure"] = json!(true);
            }
            emit(output, record)
        })
    }

    pub(super) fn candidate(
        row: &Observation,
        kind: EvidenceKind,
        history: &str,
        snapshot_id: ReadSnapshot,
    ) -> Option<SourceReference> {
        if row.identity.scope != ObservationScope::Session {
            return None;
        }
        let operation_id = match (kind, &row.kind) {
            (EvidenceKind::Instruction, ObservationKind::Message { role })
                if role == "user" && row.identity.operation_id.is_none() =>
            {
                None
            }
            (EvidenceKind::Path, ObservationKind::FileEdit(_) | ObservationKind::FileChange(_))
                if row.identity.worker_id.is_none() =>
            {
                Some(row.identity.operation_id.clone()?)
            }
            (EvidenceKind::Command, ObservationKind::Tool { category, .. })
                if category == "shell" && row.identity.worker_id.is_none() =>
            {
                Some(row.identity.operation_id.clone()?)
            }
            _ => return None,
        };
        Some(SourceReference {
            version: 1,
            history_sha256: history.into(),
            scope: Scope {
                source_id: row.harness_id.clone(),
                session_id: row.identity.session_id.clone()?,
                turn_id: Some(row.identity.turn_id.clone()?),
                worker_id: row.identity.worker_id.clone(),
            },
            kind,
            position: position(row, snapshot_id),
            operation_id,
            grant_sha256: String::new(),
        })
    }

    pub(super) fn position(row: &Observation, snapshot_id: ReadSnapshot) -> ObservationPosition {
        ObservationPosition {
            observed_at_unix_ns: row.observed_at_unix_ns,
            harness_id: row.harness_id.as_str().into(),
            event_id: row.event_id.clone(),
            revision: row.revision,
            snapshot_id,
        }
    }

    pub(super) fn hash(value: impl Serialize) -> Result<String, String> {
        serde_json::to_vec(&value)
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
            .map_err(|_| "Could not bind investigation identity".into())
    }

    pub(super) fn encoded(value: &impl Serialize) -> Result<String, String> {
        let text =
            serde_json::to_string(value).map_err(|_| "Could not encode investigation token")?;
        if text.len() > MAX_TOKEN_BYTES {
            return Err("Investigation token exceeds its size limit".into());
        }
        Ok(text)
    }

    pub(super) fn emit(output: &mut dyn Write, value: Value) -> Result<(), String> {
        serde_json::to_writer(&mut *output, &value)
            .map_err(|_| "Could not write investigation output")?;
        output
            .write_all(b"\n")
            .and_then(|()| output.flush())
            .map_err(|_| "Could not flush investigation output".into())
    }
}
