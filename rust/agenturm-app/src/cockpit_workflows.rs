use agenturm_kernel::{
    ExtensionId, Observation, ObservationKind, ObservationQuery, ObservationStore,
};
use agenturm_store::{
    ReviewKey, ReviewLedger, ReviewVersion as StoredReviewVersion, Store, WorkspaceAnchor,
    WorkspaceLedger,
};
use agenturm_ui::{
    HandoffEntry, PromptEntry, RecordedSession, RequestEntry, ReviewNoteEntry, ReviewVersion,
    TaskEntry, WorkAnchor, WorkCommand, WorkResult,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WORK_PAGE_SIZE: usize = 100;
const WORKFLOW_EVIDENCE_LIMIT: usize = 5000;
const MAX_REQUESTS: usize = 200;

fn review_notes(
    store: &impl super::HistoryReader,
    db: &Path,
    session: RecordedSession,
    note: Option<&str>,
) -> Result<WorkResult, String> {
    verify_review_snapshot(store, &session)?;
    let key = ReviewKey {
        harness_id: ExtensionId::parse(&session.harness_id).map_err(|error| error.to_string())?,
        session_id: session.session_id.clone(),
    };
    let ledger = ReviewLedger::open(db).map_err(|error| format!("Open reviews failed: {error}"))?;
    if let Some(note) = note {
        let version = StoredReviewVersion {
            receipt_sequence: session.review_version.receipt_sequence,
            retained_observations: session.review_version.retained_observations,
            read_incarnation: session.snapshot_id.incarnation(),
        };
        ledger
            .save_decision_note(&key, version, session.snapshot_id, note)
            .map_err(|error| format!("Save review decision failed: {error}"))?;
    }
    let items = ledger
        .decision_notes(&key)
        .map_err(|error| format!("Read review decisions failed: {error}"))?
        .into_iter()
        .map(|item| ReviewNoteEntry {
            version: ReviewVersion {
                receipt_sequence: item.version.receipt_sequence,
                retained_observations: item.version.retained_observations,
            },
            read_incarnation: item.version.read_incarnation,
            note: item.note,
        })
        .collect();
    Ok(WorkResult::ReviewNotes { session, items })
}

pub(super) fn verify_review_snapshot(
    store: &impl super::HistoryReader,
    session: &RecordedSession,
) -> Result<(), String> {
    let page = store
        .query_observations(&ObservationQuery {
            harness_id: Some(session.harness_id.clone()),
            session_id: Some(session.session_id.clone()),
            snapshot_id: Some(session.snapshot_id),
            ..ObservationQuery::latest(1)
        })
        .map_err(|error| format!("Read review evidence failed: {error}"))?;
    if page.observations.is_empty() {
        return Err("The selected review evidence is unavailable".into());
    }
    Ok(())
}

pub(super) fn execute(
    store: &impl super::HistoryReader,
    db: &Path,
    command: WorkCommand,
) -> Result<WorkResult, String> {
    match command {
        WorkCommand::ReviewNotes(session) => review_notes(store, db, session, None),
        WorkCommand::SaveReviewNote { session, note } => {
            review_notes(store, db, session, Some(&note))
        }
        WorkCommand::Tasks(before) => tasks(&store.workspace_ledger(db)?, before),
        WorkCommand::Handoffs(before) => handoffs(&store.workspace_ledger(db)?, before),
        WorkCommand::SaveTask {
            id,
            revision,
            anchor,
            title,
            note,
        } => {
            let ledger = store.workspace_ledger(db)?;
            if let Some(id) = id {
                if ledger
                    .update_task_fields(id, Some(&title), Some(&note), None, revision)
                    .map_err(string)?
                    .is_none()
                {
                    return Err("That task is no longer available".into());
                }
            } else {
                let anchor = anchor.map(to_store).transpose()?;
                ledger
                    .create_task(anchor.as_ref(), &title, &note)
                    .map_err(string)?;
            }
            tasks(&ledger, None)
        }
        WorkCommand::CompleteTask {
            id,
            revision,
            completed,
        } => {
            let ledger = store.workspace_ledger(db)?;
            if ledger
                .set_task_completed_if_current(id, completed, Some(revision))
                .map_err(string)?
                .is_none()
            {
                return Err("That task is no longer available".into());
            }
            tasks(&ledger, None)
        }
        WorkCommand::SaveHandoff { id, from, to, note } => {
            let ledger = store.workspace_ledger(db)?;
            if let Some(id) = id {
                if !ledger.update_handoff(id, &note).map_err(string)? {
                    return Err("That handoff is no longer available".into());
                }
            } else {
                ledger
                    .create_handoff(&to_store(from)?, &to_store(to)?, &note)
                    .map_err(string)?;
            }
            handoffs(&ledger, None)
        }
        WorkCommand::TaskLinks(id) => task_links(&store.workspace_ledger(db)?, id),
        WorkCommand::LinkTask { id, anchor } => {
            let ledger = store.workspace_ledger(db)?;
            ledger.link_task(id, &to_store(anchor)?).map_err(string)?;
            task_links(&ledger, id)
        }
        WorkCommand::UnlinkTask { id, anchor } => {
            let ledger = store.workspace_ledger(db)?;
            ledger.unlink_task(id, &to_store(anchor)?).map_err(string)?;
            task_links(&ledger, id)
        }
        WorkCommand::TaskWorkspace(id) => task_workspace(&store.workspace_ledger(db)?, id),
        WorkCommand::CaptureWorkspace { id, directory } => {
            let ledger = store.workspace_ledger(db)?;
            let captured = super::task_git::capture(Path::new(&directory))?;
            if !ledger
                .save_task_workspace_snapshot(id, &captured)
                .map_err(string)?
            {
                return Err("That task is no longer available".into());
            }
            task_workspace(&ledger, id)
        }
        WorkCommand::DeleteTask(id) => {
            let ledger = store.workspace_ledger(db)?;
            ledger.delete_task(id).map_err(string)?;
            tasks(&ledger, None)
        }
        WorkCommand::DeleteHandoff(id) => {
            let ledger = store.workspace_ledger(db)?;
            ledger.delete_handoff(id).map_err(string)?;
            handoffs(&ledger, None)
        }
        WorkCommand::ExportHandoff(id) => {
            let upper = id.checked_add(1).ok_or("Invalid handoff ID")?;
            let entry = store
                .workspace_ledger(db)?
                .list_handoffs(1, Some(upper))
                .map_err(string)?
                .items
                .into_iter()
                .next()
                .filter(|row| row.id == id)
                .ok_or("That handoff is no longer available")?;
            let destination = super::handoff_export::export(db, &entry)?;
            Ok(WorkResult::Status(format!(
                "Saved Markdown and HTML to {}",
                destination.display()
            )))
        }
        WorkCommand::Workflow => {
            let (items, partial) = pending_requests_with_seen(store, &store.workspace_ledger(db)?)?;
            Ok(WorkResult::Workflow { items, partial })
        }
        WorkCommand::Seen(item) => {
            let (owner, event, revision) = request_key(&item)?;
            store
                .workspace_ledger(db)?
                .mark_request_seen(&owner, &event, revision)
                .map_err(string)?;
            Ok(WorkResult::Seen(item))
        }
        WorkCommand::Locate(item) => {
            let (current, _) = pending_requests(store)?;
            if !current.iter().any(|row| {
                row.owner == item.owner
                    && row.event_id == item.event_id
                    && row.revision == item.revision
            }) {
                return Err(
                    "This request changed or is no longer pending. Refresh Workflow.".into(),
                );
            }
            if item.owner.worker.is_some() {
                return Err(
                    "This request belongs to a worker. Its native terminal could not be verified."
                        .into(),
                );
            }
            super::workflow_route::locate(&item.owner.source, &item.owner.session)
                .map(WorkResult::Status)
        }
        command => execute_prompt(db, None, command),
    }
}

pub(super) fn is_prompt_command(command: &WorkCommand) -> bool {
    matches!(
        command,
        WorkCommand::Prompts(_)
            | WorkCommand::RevealPrompt(_, _)
            | WorkCommand::RevealOperation(_, _)
            | WorkCommand::RevealTelemetry(_, _)
    )
}

pub(super) fn execute_prompt(
    db: &Path,
    writer: Option<&Store>,
    command: WorkCommand,
) -> Result<WorkResult, String> {
    match command {
        WorkCommand::Prompts(owner) => {
            let config = prompt_config(db)?;
            let source = ExtensionId::parse(&owner.source).map_err(string)?;
            let prompts = super::content_preview::list_user_prompts(
                db,
                &config,
                &source,
                &owner.session,
                writer,
            )?;
            Ok(WorkResult::Prompts(
                prompts
                    .into_iter()
                    .map(|prompt| {
                        let mut anchor = owner.clone();
                        anchor.turn = Some(prompt.turn_id);
                        PromptEntry {
                            anchor,
                            event_id: prompt.event_id,
                            revision: prompt.revision,
                            timestamp: prompt.observed_at_unix_ns,
                        }
                    })
                    .collect(),
            ))
        }
        WorkCommand::RevealPrompt(prompt, mut delivery) => {
            let config = prompt_config(db)?;
            let source = ExtensionId::parse(&prompt.anchor.source).map_err(string)?;
            super::content_preview::reveal_user_prompt(
                db,
                &config,
                &source,
                super::content_preview::PromptSelection {
                    session_id: &prompt.anchor.session,
                    event_id: &prompt.event_id,
                    revision: prompt.revision,
                },
                &mut delivery,
                writer,
            )?;
            Ok(WorkResult::Status(
                "Prompt shown from the verified current source".into(),
            ))
        }
        WorkCommand::RevealOperation(operation, mut delivery) => {
            let config = prompt_config(db)?;
            let source = ExtensionId::parse(&operation.source_id).map_err(string)?;
            super::content_preview::reveal_operation_detail(
                db,
                &config,
                &source,
                operation.kind,
                &agenturm_store::CodexOperationSelection {
                    session_id: &operation.session_id,
                    turn_id: Some(&operation.turn_id),
                    operation_id: &operation.operation_id,
                    event_id: &operation.event_id,
                    revision: operation.revision,
                },
                &mut delivery,
                writer,
            )?;
            Ok(WorkResult::Status(String::new()))
        }
        WorkCommand::RevealTelemetry(span, mut delivery) => {
            let selection = super::telemetry_payload_cli::modal_selection(*span)?;
            let opened;
            let store = if let Some(writer) = writer {
                writer
            } else {
                opened = Store::open_collector(db).map_err(string)?;
                &opened
            };
            store
                .with_telemetry_payload(&selection, |payload| {
                    super::telemetry_payload_cli::deliver_modal(payload, &mut delivery)
                })
                .map_err(string)?
                .ok_or("Exact retained telemetry payload is unavailable")??;
            Ok(WorkResult::Status(String::new()))
        }
        _ => Err("Unsupported prompt request".into()),
    }
}

fn prompt_config(db: &Path) -> Result<PathBuf, String> {
    let config = std::env::var_os("AGENTURM_CONTENT_CONFIG")
        .map_or_else(|| db.with_file_name("sources.json"), PathBuf::from);
    if config.is_file() {
        Ok(config)
    } else {
        Err(
            "Set AGENTURM_CONTENT_CONFIG to the collection config used to grant prompt access."
                .into(),
        )
    }
}

fn string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn tasks(ledger: &WorkspaceLedger, before: Option<u64>) -> Result<WorkResult, String> {
    let page = ledger.list_tasks(WORK_PAGE_SIZE, before).map_err(string)?;
    Ok(WorkResult::Tasks {
        items: page
            .items
            .into_iter()
            .map(|task| TaskEntry {
                id: task.id,
                revision: task.revision,
                anchor: task.anchor.map(from_store),
                title: task.title,
                note: task.note,
                completed: task.completed,
            })
            .collect(),
        older: page.next_before,
    })
}

fn task_links(ledger: &WorkspaceLedger, id: u64) -> Result<WorkResult, String> {
    let links = ledger.task_links(id, 100).map_err(string)?;
    Ok(WorkResult::TaskLinks {
        id,
        items: links.into_iter().map(from_store).collect(),
    })
}

fn task_workspace(ledger: &WorkspaceLedger, id: u64) -> Result<WorkResult, String> {
    let Some(workspace) = ledger.task_workspace_snapshot(id).map_err(string)? else {
        return Ok(WorkResult::Status(
            "No Git workspace captured. Press G to enter a repository directory.".into(),
        ));
    };
    let branch = workspace
        .branch
        .as_deref()
        .unwrap_or("Detached or unavailable");
    Ok(WorkResult::TaskWorkspace {
        root: workspace.repository_root,
        head: workspace
            .head_commit
            .unwrap_or_else(|| "No commit recorded".into()),
        summary: format!(
            "Branch  {branch}\nStaged  {}\nUnstaged  {}\nUntracked  {}",
            workspace.staged_changes, workspace.unstaged_changes, workspace.untracked_files,
        ),
    })
}

fn handoffs(ledger: &WorkspaceLedger, before: Option<u64>) -> Result<WorkResult, String> {
    let page = ledger
        .list_handoffs(WORK_PAGE_SIZE, before)
        .map_err(string)?;
    Ok(WorkResult::Handoffs {
        items: page
            .items
            .into_iter()
            .map(|handoff| HandoffEntry {
                id: handoff.id,
                from: from_store(handoff.from),
                to: from_store(handoff.to),
                note: handoff.note,
            })
            .collect(),
        older: page.next_before,
    })
}

fn to_store(anchor: WorkAnchor) -> Result<WorkspaceAnchor, String> {
    Ok(WorkspaceAnchor {
        harness_id: ExtensionId::parse(anchor.source).map_err(string)?,
        session_id: anchor.session,
        worker_id: anchor.worker,
        turn_id: anchor.turn,
    })
}

fn from_store(anchor: WorkspaceAnchor) -> WorkAnchor {
    WorkAnchor {
        source: anchor.harness_id.as_str().into(),
        session: anchor.session_id,
        worker: anchor.worker_id,
        turn: anchor.turn_id,
    }
}

fn request_key(row: &RequestEntry) -> Result<(WorkspaceAnchor, String, u32), String> {
    Ok((
        to_store(row.owner.clone())?,
        row.event_id.clone(),
        row.revision,
    ))
}

fn pending_requests(store: &impl ObservationStore) -> Result<(Vec<RequestEntry>, bool), String> {
    let page = store
        .query_observations(&ObservationQuery::latest(WORKFLOW_EVIDENCE_LIMIT))
        .map_err(string)?;
    let mut results = project_requests(&page.observations);
    let partial = page.has_more || results.len() > MAX_REQUESTS;
    results.truncate(MAX_REQUESTS);
    Ok((results, partial))
}

pub(super) fn pending_requests_with_seen(
    store: &impl ObservationStore,
    ledger: &WorkspaceLedger,
) -> Result<(Vec<RequestEntry>, bool), String> {
    let (mut items, partial) = pending_requests(store)?;
    let keys = items
        .iter()
        .map(request_key)
        .collect::<Result<Vec<_>, _>>()?;
    let seen = ledger.seen_requests(&keys).map_err(string)?;
    for (item, key) in items.iter_mut().zip(keys) {
        item.seen = seen.contains(&key);
    }
    Ok((items, partial))
}
fn project_requests(rows: &[Observation]) -> Vec<RequestEntry> {
    let mut latest_revisions = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Lifecycle { .. }))
    {
        let stored = latest_revisions
            .entry((row.harness_id.as_str(), row.event_id.as_str()))
            .or_insert(row);
        if row.revision > stored.revision {
            *stored = row;
        }
    }
    let mut latest_states = HashMap::new();
    for row in latest_revisions.into_values() {
        let Some(session) = row.identity.session_id.as_deref() else {
            continue;
        };
        let key = (
            row.harness_id.as_str(),
            session,
            row.identity.worker_id.as_deref(),
            row.identity.turn_id.as_deref(),
        );
        let stored = latest_states.entry(key).or_insert(row);
        if (row.observed_at_unix_ns, event_position(&row.event_id))
            > (stored.observed_at_unix_ns, event_position(&stored.event_id))
        {
            *stored = row;
        }
    }
    let mut results = latest_states
        .into_iter()
        .filter_map(|((source, session, worker, turn), row)| {
            let ObservationKind::Lifecycle {
                state,
                reason_code: Some(reason),
            } = &row.kind
            else {
                return None;
            };
            if state != "paused"
                || !matches!(
                    reason.as_str(),
                    "codex_approval_request"
                        | "codex_user_input_request"
                        | "kiro_interaction_pending"
                )
            {
                return None;
            }
            Some(RequestEntry {
                owner: WorkAnchor {
                    source: source.into(),
                    session: session.into(),
                    worker: worker.map(str::to_owned),
                    turn: turn.map(str::to_owned),
                },
                event_id: row.event_id.clone(),
                revision: row.revision,
                timestamp: row.observed_at_unix_ns,
                reason: reason.clone(),
                seen: false,
            })
        })
        .collect::<Vec<_>>();
    results.sort_unstable_by(|a, b| {
        (b.timestamp, &b.owner.source, &b.event_id).cmp(&(
            a.timestamp,
            &a.owner.source,
            &a.event_id,
        ))
    });
    results
}

fn event_position(event_id: &str) -> (&str, u64, &str) {
    // Codex emits the rollout offset as a decimal event ID component. Use
    // numeric order within that rollout when native timestamps are equal.
    if event_id.starts_with("codex:")
        && let Some((position, _)) = event_id.rsplit_once(':')
        && let Some((rollout, offset)) = position.rsplit_once(':')
        && let Ok(offset) = offset.parse()
    {
        return (rollout, offset, event_id);
    }
    (event_id, 0, event_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ObservationIdentity;

    #[test]
    fn a_stale_cockpit_task_cannot_overwrite_an_intervening_cli_edit() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-cockpit-task-cas-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let database = directory.join("history.sqlite");
        let store = Store::open_collector(&database).unwrap();
        let ledger = WorkspaceLedger::open(&database).unwrap();
        let task = ledger.create_task(None, "Initial", "Initial note").unwrap();
        let updated = ledger
            .update_task_fields(
                task.id,
                Some("CLI edit"),
                Some("Retain this note"),
                None,
                Some(task.revision),
            )
            .unwrap()
            .unwrap();
        let stale_save = execute(
            &store,
            &database,
            WorkCommand::SaveTask {
                id: Some(task.id),
                revision: Some(task.revision),
                anchor: None,
                title: "Stale form".into(),
                note: "Stale note".into(),
            },
        );
        assert!(stale_save.is_err());
        assert!(
            execute(
                &store,
                &database,
                WorkCommand::CompleteTask {
                    id: task.id,
                    revision: task.revision,
                    completed: true,
                }
            )
            .is_err()
        );
        let current = ledger.task(task.id).unwrap().unwrap();
        assert_eq!(current.title, "CLI edit");
        assert_eq!(current.note, "Retain this note");
        assert!(!current.completed);
        assert_eq!(current.revision, updated.revision);
        execute(
            &store,
            &database,
            WorkCommand::CompleteTask {
                id: task.id,
                revision: current.revision,
                completed: true,
            },
        )
        .unwrap();
        assert!(ledger.task(task.id).unwrap().unwrap().completed);
        drop(ledger);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workflow_requires_an_exact_pending_state_and_applies_lifecycle_corrections() {
        let row = |event: &str,
                   revision,
                   timestamp,
                   worker: Option<&str>,
                   state: &str,
                   reason: Option<&str>| {
            let mut identity = ObservationIdentity::session("session");
            identity.turn_id = Some("turn".into());
            identity.worker_id = worker.map(str::to_owned);
            Observation {
                harness_id: ExtensionId::parse("harness.codex").unwrap(),
                event_id: event.into(),
                revision,
                identity,
                observed_at_unix_ns: timestamp,
                kind: ObservationKind::Lifecycle {
                    state: state.into(),
                    reason_code: reason.map(str::to_owned),
                },
            }
        };
        let waiting = |event: &str, worker| {
            row(
                event,
                0,
                100,
                worker,
                "paused",
                Some("codex_approval_request"),
            )
        };
        let rows = [
            waiting("main", None),
            waiting("worker", Some("worker-a")),
            waiting("other", Some("worker-b")),
            row("main", 1, 90, None, "running", None),
            row("resumed", 0, 111, Some("worker-a"), "running", None),
        ];
        let pending = project_requests(&rows);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_id, "other");
        assert_eq!(pending[0].owner.worker.as_deref(), Some("worker-b"));

        let earlier = row("codex:rollout:99:lifecycle", 0, 100, None, "running", None);
        let request = waiting("codex:rollout:100:lifecycle", None);
        let resumed = row(
            "codex:rollout:1000:lifecycle",
            0,
            100,
            None,
            "running",
            None,
        );
        assert_eq!(
            project_requests(&[request.clone(), earlier.clone()])[0].event_id,
            request.event_id
        );
        assert!(project_requests(&[resumed, earlier, request]).is_empty());
    }
}
