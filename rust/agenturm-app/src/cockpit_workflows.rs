use agenturm_kernel::{ExtensionId, Observation, ObservationKind, ObservationQuery, ReadSnapshot};
use agenturm_store::{
    ReviewKey, ReviewLedger, ReviewVersion as StoredReviewVersion, Store, WorkspaceAnchor,
    WorkspaceLedger,
};
use agenturm_ui::{
    HandoffEntry, PromptEntry, RecordedSession, RequestEntry, RequestProof, ReviewNoteEntry,
    ReviewVersion, TaskEntry, WorkAnchor, WorkCommand, WorkResult,
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
    stop: &std::sync::atomic::AtomicBool,
) -> Result<WorkResult, String> {
    match command {
        WorkCommand::HostNow => {
            #[cfg(target_os = "linux")]
            let capture = store
                .local_host_data()
                .then(|| {
                    let sample = crate::cockpit_host::capture().ok()?;
                    let captured_at_unix_ns = u64::try_from(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()?
                            .as_nanos(),
                    )
                    .ok()?;
                    Some(Box::new(agenturm_ui::CurrentHostSample {
                        captured_at_unix_ns,
                        sample,
                    }))
                })
                .flatten();
            #[cfg(not(target_os = "linux"))]
            let capture = None;
            Ok(WorkResult::HostNow(capture))
        }
        WorkCommand::ExportView {
            document,
            directory,
        } => crate::view_export::export(&document, &directory),
        WorkCommand::Evaluations(request) => crate::eval_import::workspace_evals(store, *request),
        #[cfg(target_os = "linux")]
        WorkCommand::Investigate(request) if !request.kind.is_private() => {
            crate::investigate_cli::cockpit::failures(&request, store, db)
        }
        WorkCommand::HostHistory(before) => store
            .query_host_history(&ObservationQuery {
                snapshot_id: before.as_ref().map(|position| position.snapshot_id),
                before,
                ..ObservationQuery::latest(agenturm_store::MAX_HOST_HISTORY_PAGE)
            })
            .map(WorkResult::HostHistory),
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
        WorkCommand::Workflow => workflow(store, db),
        #[cfg(target_os = "linux")]
        WorkCommand::WorkflowSettings => {
            let (policy, authority) = crate::attention::workflow_settings(store, db)?;
            Ok(WorkResult::WorkflowSettings {
                policy: Box::new(policy),
                authority,
            })
        }
        #[cfg(target_os = "linux")]
        WorkCommand::ChangeWorkflowSettings {
            authority,
            expected_revision,
            change,
        } => {
            let (policy, authority) = crate::attention::change_workflow_settings(
                store,
                db,
                &authority,
                expected_revision,
                change,
            )?;
            Ok(WorkResult::WorkflowSettings {
                policy: Box::new(policy),
                authority,
            })
        }
        #[cfg(target_os = "linux")]
        WorkCommand::RetryWorkflow {
            authority,
            expected_revision,
            key,
            expected_attempt,
        } => {
            let state = crate::attention::retry_workflow_delivery(
                store,
                db,
                &authority,
                expected_revision,
                key,
                expected_attempt,
                stop,
            )?;
            Ok(WorkResult::WorkflowDeliveryUpdated(
                agenturm_kernel::workflow_notifications::WorkflowDelivery {
                    authority,
                    key,
                    policy_revision: expected_revision,
                    attempt: expected_attempt
                        .checked_add(1)
                        .ok_or("Invalid delivery attempt")?,
                    state,
                },
            ))
        }
        #[cfg(target_os = "linux")]
        WorkCommand::WorkflowAlerts => {
            let (policy, _) = crate::attention::workflow_settings(store, db)?;
            let (page, deliveries) = if policy.thresholds == Default::default() {
                (
                    crate::workflow_alerts::load(store, &policy.thresholds)?,
                    Vec::new(),
                )
            } else {
                crate::attention::workflow_alert_deliveries(store, db)?
            };
            Ok(WorkResult::WorkflowAlerts(Box::new(
                agenturm_ui::WorkflowAlertInbox { page, deliveries },
            )))
        }
        WorkCommand::SessionAttention(session) => session_attention(store, db, session),
        WorkCommand::Seen(item) => mark_seen(store, db, item),
        WorkCommand::WorkflowResume(item) => {
            verify_current_request(store, &item)?;
            let runtime = resume_runtime(&item.owner)?;
            Ok(WorkResult::WorkflowResume(Box::new(
                agenturm_ui::WorkflowResume {
                    request: item,
                    runtime,
                },
            )))
        }
        WorkCommand::Locate(item) => {
            verify_current_request(store, &item)?;
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

fn workflow(store: &impl super::HistoryReader, db: &Path) -> Result<WorkResult, String> {
    let (items, partial) = pending_requests_with_seen(store, &store.workspace_ledger(db)?)?;
    #[cfg(target_os = "linux")]
    let deliveries = crate::attention::workflow_deliveries(store, db, &items).unwrap_or_default();
    #[cfg(not(target_os = "linux"))]
    let deliveries = Vec::new();
    Ok(WorkResult::Workflow {
        items,
        partial,
        deliveries,
    })
}

pub(super) fn is_prompt_command(command: &WorkCommand) -> bool {
    matches!(
        command,
        WorkCommand::Prompts(_)
            | WorkCommand::RevealPrompt(_, _)
            | WorkCommand::RevealOperation(_, _)
            | WorkCommand::RevealTelemetry(_, _)
            | WorkCommand::RevealEvalDetail(_, _)
            | WorkCommand::RevealInvestigation { .. }
    ) || matches!(command, WorkCommand::Investigate(request) if request.kind.is_private())
}

pub(super) fn execute_prompt(
    db: &Path,
    writer: Option<&Store>,
    command: WorkCommand,
) -> Result<WorkResult, String> {
    match command {
        #[cfg(target_os = "linux")]
        command @ (WorkCommand::Investigate(_) | WorkCommand::RevealInvestigation { .. }) => {
            crate::investigate_cli::cockpit::local(command, db, writer, &prompt_config(db)?)
        }
        #[cfg(not(target_os = "linux"))]
        WorkCommand::Investigate(_) | WorkCommand::RevealInvestigation { .. } => {
            Err("Retained investigations require Linux".into())
        }
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
                            snapshot_id: prompt.snapshot_id,
                            history_identity: prompt.history_identity,
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
                    snapshot_id: prompt.snapshot_id,
                    history_identity: prompt.history_identity.as_deref(),
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
        WorkCommand::RevealEvalDetail(detail, mut delivery) => {
            let selection = super::eval_detail_cli::modal_selection(*detail);
            let opened;
            let store = if let Some(writer) = writer {
                writer
            } else {
                opened = Store::open_collector(db).map_err(string)?;
                &opened
            };
            store
                .with_eval_result_detail(&selection, |body| {
                    super::eval_detail_cli::deliver_modal(body, &mut delivery)
                })
                .map_err(string)?
                .ok_or("Exact retained evaluation detail is unavailable")??;
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

pub(super) fn request_key(row: &RequestEntry) -> Result<(WorkspaceAnchor, String, u32), String> {
    Ok((
        to_store(row.owner.clone())?,
        row.event_id.clone(),
        row.revision,
    ))
}

fn pending_requests(
    store: &impl super::HistoryReader,
    owner: Option<&WorkAnchor>,
    snapshot_id: Option<ReadSnapshot>,
) -> Result<(Vec<RequestEntry>, bool), String> {
    let page = store
        .query_observations(&ObservationQuery {
            harness_id: owner.map(|owner| owner.source.clone()),
            session_id: owner.map(|owner| owner.session.clone()),
            snapshot_id,
            ..ObservationQuery::latest(WORKFLOW_EVIDENCE_LIMIT)
        })
        .map_err(string)?;
    if snapshot_id.is_some_and(|snapshot| snapshot != page.snapshot_id) {
        return Err("The selected request history snapshot changed".into());
    }
    let proof = RequestProof {
        snapshot_id: page.snapshot_id,
        history_identity: store.workspace_history_identity(),
    };
    let mut results = project_requests(&page.observations, &proof);
    let native =
        store.query_native_attention(&agenturm_store::native_attention::NativeAttentionQuery {
            source_id: owner
                .map(|value| ExtensionId::parse(&value.source))
                .transpose()
                .map_err(string)?,
            session_id: owner.map(|value| value.session.clone()),
            snapshot_id: Some(page.snapshot_id),
            limit: MAX_REQUESTS,
            include_receiver_clock: false,
        })?;
    if native.snapshot_id != page.snapshot_id {
        return Err("Native attention history changed; refresh Workflow".into());
    }
    for request in native.pending {
        results.push(RequestEntry {
            owner: WorkAnchor {
                source: request.scope.source_id,
                session: request.scope.session_id,
                turn: Some(request.scope.turn_id),
                worker: request.scope.worker_id,
            },
            event_id: request.event_id,
            revision: 0,
            timestamp: request.receipt_time_unix_ns,
            reason: if request.kind
                == agenturm_kernel::native_attention::NativeAttentionKind::ApprovalRequested
            {
                "native_approval_request"
            } else {
                "native_user_input_request"
            }
            .into(),
            seen: false,
            proof: proof.clone(),
        });
    }
    results.sort_by(|a, b| {
        (b.timestamp, &b.owner.source, &b.event_id).cmp(&(
            a.timestamp,
            &a.owner.source,
            &a.event_id,
        ))
    });
    let partial = page.has_more || native.truncated || results.len() > MAX_REQUESTS;
    results.truncate(MAX_REQUESTS);
    Ok((results, partial))
}

pub(super) fn pending_requests_with_seen(
    store: &impl super::HistoryReader,
    ledger: &WorkspaceLedger,
) -> Result<(Vec<RequestEntry>, bool), String> {
    requests_with_seen(store, ledger, None, None)
}

pub(super) fn roster_pending(
    store: &impl super::HistoryReader,
    db: &Path,
    snapshot_id: ReadSnapshot,
) -> Result<(Vec<RequestEntry>, bool), String> {
    requests_with_seen(store, &store.workspace_ledger(db)?, None, Some(snapshot_id))
}

fn requests_with_seen(
    store: &impl super::HistoryReader,
    ledger: &WorkspaceLedger,
    owner: Option<&WorkAnchor>,
    snapshot_id: Option<ReadSnapshot>,
) -> Result<(Vec<RequestEntry>, bool), String> {
    let (mut items, partial) = pending_requests(store, owner, snapshot_id)?;
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

fn session_attention(
    store: &impl super::HistoryReader,
    db: &Path,
    session: RecordedSession,
) -> Result<WorkResult, String> {
    if session.history_identity.is_none()
        || session.history_identity != store.workspace_history_identity()
    {
        return Err(
            "The selected session no longer belongs to this history. Refresh Agents.".into(),
        );
    }
    let ledger = store.workspace_ledger(db)?;
    let (mut items, partial) = requests_with_seen(
        store,
        &ledger,
        Some(&WorkAnchor::from(&session)),
        Some(session.snapshot_id),
    )?;
    if partial {
        return Err("The selected session's pending requests could not be read completely".into());
    }
    items.retain(|item| !item.seen);
    let (mut current, partial) =
        requests_with_seen(store, &ledger, Some(&WorkAnchor::from(&session)), None)?;
    if partial {
        return Err("The current session's pending requests could not be read completely".into());
    }
    current.retain(|item| !item.seen);
    match current.as_slice() {
        [] => Ok(WorkResult::Status(
            "No unseen pending request for this session".into(),
        )),
        [item] if items.len() == 1 && same_request(&items[0], item) => {
            mark_seen(store, db, item.clone())
        }
        [item] if !items.iter().any(|prior| same_request(prior, item)) => {
            Err("The selected session's request changed. Refresh Agents.".into())
        }
        _ => Ok(WorkResult::AttentionRequests(current)),
    }
}

fn same_request(left: &RequestEntry, right: &RequestEntry) -> bool {
    left.owner == right.owner
        && left.event_id == right.event_id
        && left.revision == right.revision
        && left.proof.history_identity == right.proof.history_identity
        && left.proof.snapshot_id.incarnation() == right.proof.snapshot_id.incarnation()
}

fn resume_runtime(owner: &WorkAnchor) -> Result<agenturm_ui::ResumeRuntime, String> {
    if owner.worker.is_some()
        || owner.session.len() > 200
        || !owner
            .session
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !owner
            .session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(
            "A resume command requires an exact main session with a safe native identifier".into(),
        );
    }
    for (source, runtime) in [
        ("harness.codex", agenturm_ui::ResumeRuntime::Codex),
        ("harness.claude", agenturm_ui::ResumeRuntime::Claude),
    ] {
        if owner.source == source
            || owner
                .source
                .strip_prefix(source)
                .is_some_and(|rest| rest.starts_with('.'))
        {
            return Ok(runtime);
        }
    }
    Err("This provider does not have a supported native resume command".into())
}

fn verify_current_request(
    store: &impl super::HistoryReader,
    item: &RequestEntry,
) -> Result<(), String> {
    if item.proof.history_identity.is_none()
        || item.proof.history_identity != store.workspace_history_identity()
    {
        return Err("The selected request no longer belongs to this history".into());
    }
    let matches = |candidate: &RequestEntry| same_request(item, candidate);
    let (original, original_partial) =
        pending_requests(store, Some(&item.owner), Some(item.proof.snapshot_id))?;
    let (current, current_partial) = pending_requests(store, Some(&item.owner), None)?;
    if original_partial
        || current_partial
        || !original.iter().any(matches)
        || !current.iter().any(matches)
    {
        return Err("This exact request changed, is no longer pending, or its history is incomplete. Refresh Agents or Workflow.".into());
    }
    Ok(())
}

fn mark_seen(
    store: &impl super::HistoryReader,
    db: &Path,
    item: RequestEntry,
) -> Result<WorkResult, String> {
    let ledger = store.workspace_ledger(db)?;
    verify_current_request(store, &item)?;
    // The local reader may still have the previous inode open after an
    // external replacement. Match the ledger's current history before writing.
    if store.session_title_store().is_some() {
        let current = Store::open_read_only(db).map_err(string)?;
        if Some(current.history_identity().map_err(string)?) != item.proof.history_identity
            || current.read_incarnation().map_err(string)? != item.proof.snapshot_id.incarnation()
        {
            return Err("The selected request history changed".into());
        }
    }
    let (owner, event, revision) = request_key(&item)?;
    ledger
        .mark_request_seen(&owner, &event, revision)
        .map_err(string)?;
    Ok(WorkResult::Seen(item))
}

pub(super) fn project_requests(rows: &[Observation], proof: &RequestProof) -> Vec<RequestEntry> {
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
                proof: proof.clone(),
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

    fn execute(
        store: &impl crate::HistoryReader,
        db: &Path,
        command: WorkCommand,
    ) -> Result<WorkResult, String> {
        super::execute(
            store,
            db,
            command,
            &std::sync::atomic::AtomicBool::new(false),
        )
    }

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
    #[cfg(target_os = "linux")]
    fn seen_uses_scoped_current_request_and_preserves_pending_lifecycle() {
        use agenturm_kernel::ObservationStore;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "agenturm-exact-seen-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let db = dir.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let entry = |source: &str, worker: &str, event: &str, revision, at, state: &str| {
            let mut identity = ObservationIdentity::session("same-session");
            identity.worker_id = Some(worker.into());
            identity.turn_id = Some("recorded-turn".into());
            Observation {
                harness_id: ExtensionId::parse(source).unwrap(),
                event_id: event.into(),
                revision,
                identity,
                observed_at_unix_ns: at,
                kind: ObservationKind::Lifecycle {
                    state: state.into(),
                    reason_code: (state == "paused").then(|| "codex_approval_request".into()),
                },
            }
        };
        let main = "harness.codex";
        let sibling = "harness.sibling";
        store
            .commit(
                main,
                None,
                "one",
                &[
                    entry(main, "worker-a", "approval-a", 0, 10, "paused"),
                    entry(main, "worker-b", "approval-b", 0, 11, "paused"),
                ],
            )
            .unwrap();
        store
            .commit(
                sibling,
                None,
                "one",
                &[entry(sibling, "worker-a", "approval-a", 0, 12, "paused")],
            )
            .unwrap();
        let catalog = super::super::load_session_catalog(&store, &db, None).unwrap();
        let selected = catalog
            .sessions
            .into_iter()
            .find(|row| row.harness_id == main)
            .unwrap();
        assert_eq!(selected.pending_requests.len(), 2);
        let WorkResult::AttentionRequests(choices) =
            execute(&store, &db, WorkCommand::SessionAttention(selected.clone())).unwrap()
        else {
            panic!("multiple workers require a chooser")
        };
        assert_eq!(choices.len(), 2);
        assert!(
            choices
                .iter()
                .all(|item| item.owner.source == main && !item.seen)
        );
        assert!(
            pending_requests_with_seen(&store, &WorkspaceLedger::open(&db).unwrap())
                .unwrap()
                .0
                .iter()
                .all(|item| !item.seen)
        );
        let approved = choices
            .into_iter()
            .find(|item| item.event_id == "approval-a")
            .unwrap();
        assert!(matches!(
            execute(&store, &db, WorkCommand::Seen(approved.clone())),
            Ok(WorkResult::Seen(_))
        ));
        let ledger = WorkspaceLedger::open(&db).unwrap();
        let pending = pending_requests_with_seen(&store, &ledger).unwrap().0;
        assert_eq!(
            pending.len(),
            3,
            "a seen receipt does not resolve the request"
        );
        assert!(
            pending.iter().any(|item| item.owner.source == main
                && item.event_id == "approval-a"
                && item.seen)
        );
        assert!(pending.iter().any(|item| item.owner.source == sibling
            && item.event_id == "approval-a"
            && !item.seen));

        store
            .commit(
                main,
                Some("one"),
                "two",
                &[entry(main, "worker-a", "approval-a", 1, 10, "paused")],
            )
            .unwrap();
        assert!(execute(&store, &db, WorkCommand::Seen(approved)).is_err());
        let pending = pending_requests_with_seen(&store, &ledger).unwrap().0;
        let revised = pending
            .into_iter()
            .find(|item| item.owner.source == main && item.event_id == "approval-a")
            .unwrap();
        assert_eq!(revised.revision, 1);
        assert!(!revised.seen);

        let archive = dir.join("backup");
        let restored_dir = dir.join("restored");
        Store::backup_workspace_bundle(&db, &archive).unwrap();
        Store::restore_workspace_bundle(&archive, &restored_dir).unwrap();
        let restored_path = restored_dir.join("history.sqlite");
        let restored = Store::open_read_only(&restored_path).unwrap();
        assert_eq!(
            restored.history_identity().unwrap(),
            store.history_identity().unwrap()
        );
        assert!(
            execute(
                &restored,
                &restored_path,
                WorkCommand::Seen(revised.clone())
            )
            .is_err()
        );
        assert!(
            execute(
                &restored,
                &restored_path,
                WorkCommand::SessionAttention(selected)
            )
            .is_err()
        );

        store
            .commit(
                main,
                Some("two"),
                "three",
                &[entry(main, "worker-a", "continued", 0, 20, "running")],
            )
            .unwrap();
        assert!(execute(&store, &db, WorkCommand::Seen(revised)).is_err());
        let pending = pending_requests_with_seen(&store, &ledger).unwrap().0;
        assert!(
            !pending
                .iter()
                .any(|item| item.owner.source == main && item.event_id == "approval-a")
        );
        drop(restored);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn workflow_requires_an_exact_pending_state_and_applies_lifecycle_corrections() {
        let proof = RequestProof {
            snapshot_id: 1.into(),
            history_identity: None,
        };
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
        let pending = project_requests(&rows, &proof);
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
            project_requests(&[request.clone(), earlier.clone()], &proof)[0].event_id,
            request.event_id
        );
        assert!(project_requests(&[resumed, earlier, request], &proof).is_empty());
    }
}
