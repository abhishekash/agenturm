use agenturm_kernel::{ExtensionId, Observation, ObservationKind, ObservationQuery};
use agenturm_store::{Store, WorkspaceAnchor, WorkspaceLedger};
use agenturm_ui::{
    HandoffEntry, PromptEntry, RequestEntry, TaskEntry, WorkAnchor, WorkCommand, WorkResult,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WORK_PAGE_SIZE: usize = 100;
const WORKFLOW_EVIDENCE_LIMIT: usize = 5000;
const MAX_REQUESTS: usize = 200;

pub(super) fn execute(
    store: &Store,
    db: &Path,
    command: WorkCommand,
) -> Result<WorkResult, String> {
    match command {
        WorkCommand::Tasks(before) => tasks(&ledger(db)?, before),
        WorkCommand::Handoffs(before) => handoffs(&ledger(db)?, before),
        WorkCommand::SaveTask {
            id,
            anchor,
            title,
            note,
        } => {
            let ledger = ledger(db)?;
            if let Some(id) = id {
                if !ledger.update_task(id, &title, &note).map_err(string)? {
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
        WorkCommand::CompleteTask { id, completed } => {
            let ledger = ledger(db)?;
            if !ledger.set_task_completed(id, completed).map_err(string)? {
                return Err("That task is no longer available".into());
            }
            tasks(&ledger, None)
        }
        WorkCommand::SaveHandoff { id, from, to, note } => {
            let ledger = ledger(db)?;
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
        WorkCommand::TaskLinks(id) => task_links(&ledger(db)?, id),
        WorkCommand::LinkTask { id, anchor } => {
            let ledger = ledger(db)?;
            ledger.link_task(id, &to_store(anchor)?).map_err(string)?;
            task_links(&ledger, id)
        }
        WorkCommand::UnlinkTask { id, anchor } => {
            let ledger = ledger(db)?;
            ledger.unlink_task(id, &to_store(anchor)?).map_err(string)?;
            task_links(&ledger, id)
        }
        WorkCommand::TaskWorkspace(id) => task_workspace(&ledger(db)?, id),
        WorkCommand::CaptureWorkspace { id, directory } => {
            let ledger = ledger(db)?;
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
            let ledger = ledger(db)?;
            ledger.delete_task(id).map_err(string)?;
            tasks(&ledger, None)
        }
        WorkCommand::DeleteHandoff(id) => {
            let ledger = ledger(db)?;
            ledger.delete_handoff(id).map_err(string)?;
            handoffs(&ledger, None)
        }
        WorkCommand::ExportHandoff(id) => {
            let upper = id.checked_add(1).ok_or("Invalid handoff ID")?;
            let entry = ledger(db)?
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
            let (mut items, partial) = pending_requests(store)?;
            let keys = items
                .iter()
                .map(request_key)
                .collect::<Result<Vec<_>, _>>()?;
            let seen = ledger(db)?.seen_requests(&keys).map_err(string)?;
            for (item, key) in items.iter_mut().zip(keys) {
                item.seen = seen.contains(&key);
            }
            Ok(WorkResult::Workflow { items, partial })
        }
        WorkCommand::Seen(item) => {
            let (owner, event, revision) = request_key(&item)?;
            ledger(db)?
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
        WorkCommand::Prompts(owner) => {
            let config = prompt_config(db)?;
            let source = ExtensionId::parse(&owner.source).map_err(string)?;
            let prompts =
                super::pi_preview::list_pi_user_prompts(db, &config, &source, &owner.session)?;
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
            super::pi_preview::reveal_pi_user_prompt(
                db,
                &config,
                &source,
                &prompt.anchor.session,
                &prompt.event_id,
                prompt.revision,
                &mut delivery,
            )?;
            Ok(WorkResult::Status(
                "Prompt shown from the verified current source".into(),
            ))
        }
    }
}

fn prompt_config(db: &Path) -> Result<PathBuf, String> {
    let config = std::env::var_os("AGENTURM_CONTENT_CONFIG")
        .map_or_else(|| db.with_file_name("sources.json"), PathBuf::from);
    if config.is_file() {
        Ok(config)
    } else {
        Err(
            "Set AGENTURM_CONTENT_CONFIG to the collection config used to grant Pi prompt access."
                .into(),
        )
    }
}

fn ledger(db: &Path) -> Result<WorkspaceLedger, String> {
    WorkspaceLedger::open(db).map_err(string)
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

fn pending_requests(store: &Store) -> Result<(Vec<RequestEntry>, bool), String> {
    let page = store
        .query_observations(&ObservationQuery::latest(WORKFLOW_EVIDENCE_LIMIT))
        .map_err(string)?;
    let mut results = project_requests(&page.observations);
    let partial = page.has_more || results.len() > MAX_REQUESTS;
    results.truncate(MAX_REQUESTS);
    Ok((results, partial))
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
