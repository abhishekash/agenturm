use agenturm_kernel::{ExtensionId, Observation, ObservationKind, ObservationQuery, ReadSnapshot};
use agenturm_store::{
    ReviewKey, ReviewVersion as StoredReviewVersion, Store, TaskBriefUpdate, TaskDecisionInput,
    TaskDecisionKind, WorkspaceAnchor, WorkspaceLedger,
};
use agenturm_ui::{
    HandoffEntry, PromptEntry, RecordedSession, RequestEntry, RequestProof, ReviewNoteEntry,
    ReviewVersion, TaskDecisionEntry, TaskEntry, WorkAnchor, WorkCommand, WorkResult,
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
    let ledger = store.review_ledger(db, &session)?;
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
    if session.history_identity != store.workspace_history_identity() {
        return Err("The selected Review belongs to a different history".into());
    }
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
        WorkCommand::HostNow => crate::cockpit_host::host_now(store),
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
        WorkCommand::ReviewReceipt(request) => {
            let text = crate::review_receipt_for_scope(
                store,
                db,
                &request.query,
                request.format.as_str(),
                request.directory.as_deref(),
            )?;
            let mut scope = format!(
                "{} / {}",
                request.query.anchor.source, request.query.anchor.session
            );
            if let Some(turn) = &request.query.anchor.turn {
                scope.push_str(&format!(" / turn {turn}"));
            }
            if let Some(worker) = &request.query.anchor.worker {
                scope.push_str(&format!(" / worker {worker}"));
            }
            if let Some(directory) = &request.directory {
                scope.push_str(&format!(" / exported to {directory}"));
            } else {
                scope.push_str(" / loaded review evidence");
            }
            Ok(WorkResult::ReviewDocument {
                title: "Review receipt".into(),
                text,
                scope,
            })
        }
        WorkCommand::ReviewExcerpts(query) => {
            let text = crate::review_receipt_for_scope(store, db, &query, "json", None)?;
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|_| "Cannot read the review excerpt index")?;
            let items = value["excerpts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    Some(agenturm_ui::ReviewExcerpt {
                        kind: item["kind"].as_str()?.to_owned(),
                        label: item["label"].as_str()?.to_owned(),
                        source: item["source"].as_str()?.to_owned(),
                    })
                })
                .collect();
            Ok(WorkResult::ReviewExcerpts {
                anchor: query.anchor,
                items,
            })
        }
        WorkCommand::DraftReviewTask {
            anchor,
            title,
            note,
        } => Ok(WorkResult::TaskDraft {
            id: None,
            revision: None,
            anchor: Some(anchor),
            title,
            note,
        }),
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
        WorkCommand::TaskBrief(id) => task_brief(&store.workspace_ledger(db)?, id),
        WorkCommand::SaveTaskBrief {
            id,
            expected_revision,
            intent,
            acceptance_criteria,
            constraints,
            open_questions,
            next_action,
        } => {
            let ledger = store.workspace_ledger(db)?;
            ledger
                .update_task_brief(
                    id,
                    &TaskBriefUpdate {
                        intent: Some(intent),
                        acceptance_criteria: Some(acceptance_criteria),
                        constraints: Some(constraints),
                        open_questions: Some(open_questions),
                        next_action: Some(next_action),
                    },
                    expected_revision,
                )
                .map_err(string)?
                .ok_or("That task is no longer available")?;
            task_brief(&ledger, id)
        }
        WorkCommand::TaskDecisions(id) => task_decisions(&store.workspace_ledger(db)?, id),
        WorkCommand::AddTaskDecision {
            id,
            expected_revision,
            kind,
            text,
            rationale,
            scope,
            supersedes_id,
        } => {
            let ledger = store.workspace_ledger(db)?;
            let kind = match kind.as_str() {
                "accepted" => TaskDecisionKind::Accepted,
                "rejected" => TaskDecisionKind::Rejected,
                _ => return Err("Choose accepted or rejected".into()),
            };
            ledger
                .add_task_decision(
                    id,
                    &TaskDecisionInput {
                        kind,
                        text,
                        rationale,
                        scope,
                        supersedes_id,
                    },
                    expected_revision,
                )
                .map_err(string)?
                .ok_or("That task is no longer available")?;
            task_decisions(&ledger, id)
        }
        WorkCommand::PreviewHandoff { id, format } => {
            if !matches!(format.as_str(), "markdown" | "html") {
                return Err("Choose Markdown or HTML for a saved handoff".into());
            }
            let ledger = store.workspace_ledger(db)?;
            let handoff = saved_handoff(&ledger, id)?;
            let html = format == "html";
            let text = super::handoff_export::render_saved(&handoff, html);
            Ok(WorkResult::HandoffPreview { id, format, text })
        }
        WorkCommand::ExportHandoffAs { id, directory } => {
            let ledger = store.workspace_ledger(db)?;
            let handoff = saved_handoff(&ledger, id)?;
            let destination = super::handoff_export::export_documents(
                Path::new(&directory),
                &[
                    (
                        "handoff.md",
                        &super::handoff_export::render_saved(&handoff, false),
                    ),
                    (
                        "handoff.html",
                        &super::handoff_export::render_saved(&handoff, true),
                    ),
                ],
            )
            .map_err(string)?;
            Ok(WorkResult::Status(format!(
                "Saved Markdown and HTML to {}",
                destination.display()
            )))
        }
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
            | WorkCommand::EnablePromptHistory(_)
            | WorkCommand::PromptPage { .. }
            | WorkCommand::RevealPrompt(_, _)
            | WorkCommand::RevealOperation(_, _)
            | WorkCommand::RevealReviewEdit(_, _)
            | WorkCommand::RevealTelemetry(_, _)
            | WorkCommand::RevealTelemetryLog(_, _)
            | WorkCommand::RevealEvalDetail(_, _)
            | WorkCommand::RevealFailedEvalFinding { .. }
            | WorkCommand::RevealInvestigation { .. }
    ) || matches!(command, WorkCommand::Investigate(request) if request.kind.is_private())
}

fn saved_handoff(
    ledger: &WorkspaceLedger,
    id: u64,
) -> Result<agenturm_store::SavedHandoff, String> {
    let upper = id.checked_add(1).ok_or("Invalid handoff ID")?;
    ledger
        .list_handoffs(1, Some(upper))
        .map_err(string)?
        .items
        .into_iter()
        .next()
        .filter(|row| row.id == id)
        .ok_or_else(|| "That handoff is no longer available".into())
}

fn prompt_history_page(
    db: &Path,
    writer: Option<&Store>,
    owner: WorkAnchor,
    before: Option<agenturm_ui::PromptHistoryCursor>,
    config: Result<&dyn super::content_preview::ContentSourceConfig, String>,
) -> Result<WorkResult, String> {
    Ok(prompt_history_with(owner, before, |owner, cursor| {
        let config = config.map_err(crate::prompt_history::PromptHistoryError::Unavailable)?;
        let source = ExtensionId::parse(&owner.source)
            .map_err(|_| crate::prompt_history::PromptHistoryError::MissingIdentity)?;
        crate::prompt_history::load_page(
            db,
            config,
            crate::prompt_history::PromptReadScope {
                source: &source,
                session: &owner.session,
                worker: owner.worker.as_deref(),
                turn: owner.turn.as_deref(),
            },
            cursor,
            crate::prompt_history::DEFAULT_PROMPT_PAGE,
            writer,
        )
    }))
}

/// Shared local/broker page policy. Transport readers supply only their
/// existing grant-checked references; scope and cursor proofs stay identical.
pub(super) fn prompt_history_with(
    owner: WorkAnchor,
    before: Option<agenturm_ui::PromptHistoryCursor>,
    read: impl FnOnce(
        &WorkAnchor,
        Option<&crate::prompt_history::PromptCursor>,
    ) -> Result<
        crate::prompt_history::PromptPage,
        crate::prompt_history::PromptHistoryError,
    >,
) -> WorkResult {
    use agenturm_ui::{PromptAvailability as Availability, PromptHistoryPage};
    let mut result = PromptHistoryPage {
        anchor: owner.clone(),
        entries: Vec::new(),
        retained: 0,
        retained_cap: agenturm_store::MAX_PROMPT_REFERENCES,
        limited: false,
        older: None,
        availability: Availability::Granted,
    };
    if owner.session.trim().is_empty() {
        result.availability = Availability::MissingIdentity;
        return WorkResult::PromptHistory(result);
    }
    let cursor = before
        .as_ref()
        .map(|cursor| crate::prompt_history::PromptCursor {
            observed_at_unix_ns: cursor.timestamp,
            event_id: cursor.event_id.clone(),
        });
    let page = read(&owner, cursor.as_ref());
    match page {
        Ok(page) => {
            if let Some(before) = &before
                && (page.requests.is_empty()
                    || page.requests.iter().any(|entry| {
                        entry.snapshot_id != Some(before.snapshot_id)
                            || entry.history_identity.as_deref() != Some(&before.history_identity)
                    }))
            {
                result.availability = Availability::Stale(
                    "The retained prompt window changed; reopen Prompts".into(),
                );
                return WorkResult::PromptHistory(result);
            }
            result.retained = page.retained;
            result.limited = page.limited;
            result.retained_cap = page.retained_cap;
            result.older = page.older.and_then(|cursor| {
                let proof = page.requests.last()?;
                Some(agenturm_ui::PromptHistoryCursor {
                    timestamp: cursor.observed_at_unix_ns,
                    event_id: cursor.event_id,
                    snapshot_id: proof.snapshot_id?,
                    history_identity: proof.history_identity.clone()?,
                })
            });
            result.entries = page
                .requests
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
                .collect();
        }
        Err(crate::prompt_history::PromptHistoryError::MissingIdentity) => {
            result.availability = Availability::MissingIdentity
        }
        Err(crate::prompt_history::PromptHistoryError::ApprovalRequired(reason)) => {
            result.availability = Availability::ApprovalRequired(reason)
        }
        Err(crate::prompt_history::PromptHistoryError::Denied(reason)) => {
            result.availability = Availability::Denied(reason)
        }
        Err(crate::prompt_history::PromptHistoryError::Unavailable(reason)) => {
            result.availability = Availability::Unavailable(reason)
        }
    }
    WorkResult::PromptHistory(result)
}

pub(super) fn execute_prompt(
    db: &Path,
    writer: Option<&Store>,
    command: WorkCommand,
) -> Result<WorkResult, String> {
    execute_prompt_with_config(db, writer, command, None)
}

pub(super) fn execute_prompt_with_config(
    db: &Path,
    writer: Option<&Store>,
    command: WorkCommand,
    active: Option<&dyn super::content_preview::ContentSourceConfig>,
) -> Result<WorkResult, String> {
    let file = if active.is_none() {
        prompt_config(db)
    } else {
        Ok(PathBuf::new())
    };
    let config = || match active {
        Some(active) => Ok(active),
        None => file
            .as_ref()
            .map(|path| path as &dyn super::content_preview::ContentSourceConfig)
            .map_err(Clone::clone),
    };
    match command {
        #[cfg(target_os = "linux")]
        command @ (WorkCommand::Investigate(_) | WorkCommand::RevealInvestigation { .. }) => {
            crate::investigate_cli::cockpit::local(command, db, writer, config()?)
        }
        #[cfg(not(target_os = "linux"))]
        WorkCommand::Investigate(_) | WorkCommand::RevealInvestigation { .. } => {
            Err("Retained investigations require Linux".into())
        }
        #[cfg(target_os = "linux")]
        WorkCommand::EnablePromptHistory(owner) => {
            if writer.is_some() {
                return Err("Enable prompt history through the active collector".into());
            }
            let config = config()?;
            let mut store = Store::open_collector(db).map_err(string)?;
            let scope = super::content_preview::prompt_grant_scope(&store, config, &owner)?;
            store.grant_content(&scope).map_err(string)?;
            prompt_history_page(db, Some(&store), owner, None, Ok(config))
        }
        #[cfg(not(target_os = "linux"))]
        WorkCommand::EnablePromptHistory(_) => Err("Prompt history is available on Linux".into()),
        WorkCommand::Prompts(owner) => prompt_history_page(db, writer, owner, None, config()),
        WorkCommand::PromptPage { anchor, before } => {
            prompt_history_page(db, writer, anchor, before, config())
        }
        WorkCommand::RevealPrompt(prompt, mut delivery) => {
            let config = config()?;
            let source = ExtensionId::parse(&prompt.anchor.source).map_err(string)?;
            super::content_preview::reveal_user_prompt(
                db,
                config,
                &source,
                super::content_preview::PromptSelection {
                    session_id: &prompt.anchor.session,
                    worker_id: prompt.anchor.worker.as_deref(),
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
            let config = config()?;
            let source = ExtensionId::parse(&operation.source_id).map_err(string)?;
            super::content_preview::reveal_operation_detail(
                db,
                config,
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
        WorkCommand::RevealReviewEdit(edit, mut delivery) => {
            super::content_preview::reveal_review_edit(
                db,
                config()?,
                &edit,
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
        WorkCommand::RevealTelemetryLog(log, mut delivery) => {
            let selection = super::findings::log_selection(*log);
            let opened;
            let store = if let Some(writer) = writer {
                writer
            } else {
                opened = Store::open_collector(db).map_err(string)?;
                &opened
            };
            store
                .with_telemetry_log_text(&selection, |body| {
                    super::findings::deliver_log_text(body, &mut delivery)
                })
                .map_err(string)?
                .ok_or("Captured log text is unavailable")??;
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
        WorkCommand::RevealFailedEvalFinding {
            detail,
            history_identity,
            evidence_snapshot,
            mut delivery,
        } => {
            let selection = agenturm_store::EvalFindingSelection {
                detail: super::eval_detail_cli::modal_selection(*detail),
                history_identity,
                evidence_snapshot,
            };
            let opened;
            let store = if let Some(writer) = writer {
                writer
            } else {
                opened = Store::open_collector(db).map_err(string)?;
                &opened
            };
            store
                .with_failed_eval_finding(&selection, |body| {
                    super::eval_detail_cli::deliver_modal(body, &mut delivery)
                })
                .map_err(string)?
                .ok_or("Failed evaluation changed or detail is unavailable")??;
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

fn task_brief(ledger: &WorkspaceLedger, id: u64) -> Result<WorkResult, String> {
    let (task, brief) = ledger
        .task_brief(id)
        .map_err(string)?
        .ok_or("That task is no longer available")?;
    Ok(WorkResult::TaskBrief {
        id: task.id,
        revision: Some(task.revision),
        intent: brief.intent,
        acceptance_criteria: brief.acceptance_criteria,
        constraints: brief.constraints,
        open_questions: brief.open_questions,
        next_action: brief.next_action,
    })
}

fn task_decisions(ledger: &WorkspaceLedger, id: u64) -> Result<WorkResult, String> {
    let (task, decisions) = ledger
        .task_decisions(id)
        .map_err(string)?
        .ok_or("That task is no longer available")?;
    Ok(WorkResult::TaskDecisions {
        id: task.id,
        revision: Some(task.revision),
        items: decisions
            .into_iter()
            .map(|item| TaskDecisionEntry {
                id: item.id,
                kind: item.decision.kind.as_str().to_owned(),
                text: item.decision.text,
                rationale: item.decision.rationale,
                scope: item.decision.scope,
                supersedes: item.decision.supersedes_id,
            })
            .collect(),
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

pub(crate) fn verify_current_request(
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
    use crate::HistoryReader;
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

    fn scratch(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-parity-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        directory
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn active_prompt_authority_does_not_load_a_separate_config_file() {
        struct Active(std::cell::Cell<usize>);
        impl super::super::content_preview::ContentSourceConfig for Active {
            fn default_prompt_history(&self) -> bool {
                true
            }

            fn resolve_content_source(
                &self,
                _: &ExtensionId,
            ) -> Result<agenturm_content::ResolvedSource, String> {
                self.0.set(self.0.get() + 1);
                Err("Active source is unavailable".into())
            }
        }
        let active = Active(std::cell::Cell::new(0));
        let root =
            std::env::temp_dir().join(format!("agenturm-active-authority-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("history.sqlite");
        let store = Store::open_collector(&db).unwrap();
        let result = execute_prompt_with_config(
            &db,
            Some(&store),
            WorkCommand::Prompts(WorkAnchor {
                source: "harness.codex.active".into(),
                session: "active".into(),
                worker: None,
                turn: None,
            }),
            Some(&active),
        )
        .unwrap();
        let WorkResult::PromptHistory(page) = result else {
            panic!("prompt page")
        };
        assert_eq!(active.0.get(), 1);
        assert_eq!(
            page.availability,
            agenturm_ui::PromptAvailability::Denied("Active source is unavailable".into())
        );
        assert!(!root.join("sources.json").exists());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_handoff_preview_and_export_never_resolve_a_colliding_task_id() {
        use agenturm_kernel::ObservationStore;
        let directory = scratch("handoff-identity");
        let db = directory.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let observation = |session: &str| Observation {
            harness_id: ExtensionId::parse("harness.fixture").unwrap(),
            event_id: session.into(),
            revision: 0,
            identity: ObservationIdentity::session(session),
            observed_at_unix_ns: 1,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };
        store
            .commit(
                "harness.fixture",
                None,
                "seed",
                &[observation("from-session"), observation("to-session")],
            )
            .unwrap();
        let ledger = store.workspace_ledger(&db).unwrap();
        let task = ledger
            .create_task(None, "UNRELATED_TASK_MARKER", "UNRELATED_TASK_NOTE")
            .unwrap();
        let from = WorkspaceAnchor {
            harness_id: ExtensionId::parse("harness.fixture").unwrap(),
            session_id: "from-session".into(),
            worker_id: None,
            turn_id: None,
        };
        let to = WorkspaceAnchor {
            session_id: "to-session".into(),
            ..from.clone()
        };
        let handoff = ledger
            .create_handoff(&from, &to, "SELECTED_HANDOFF_MARKER")
            .unwrap();
        assert_eq!(
            task.id, handoff.id,
            "independent numeric namespaces must actually collide in this regression"
        );
        let WorkResult::HandoffPreview { text, .. } = execute(
            &store,
            &db,
            WorkCommand::PreviewHandoff {
                id: handoff.id,
                format: "markdown".into(),
            },
        )
        .unwrap() else {
            panic!("preview");
        };
        assert!(
            text.contains("SELECTED_HANDOFF_MARKER")
                && text.contains("from-session")
                && text.contains("to-session")
        );
        assert!(!text.contains("UNRELATED_TASK"));
        let destination = directory.join("export");
        execute(
            &store,
            &db,
            WorkCommand::ExportHandoffAs {
                id: handoff.id,
                directory: destination.display().to_string(),
            },
        )
        .unwrap();
        for name in ["handoff.md", "handoff.html"] {
            let text = std::fs::read_to_string(destination.join(name)).unwrap();
            assert!(text.contains("SELECTED_HANDOFF_MARKER"));
            assert!(!text.contains("UNRELATED_TASK"));
        }
        assert!(
            execute(
                &store,
                &db,
                WorkCommand::ExportHandoffAs {
                    id: handoff.id,
                    directory: destination.display().to_string()
                }
            )
            .is_err()
        );
        ledger.delete_handoff(handoff.id).unwrap();
        assert!(
            execute(
                &store,
                &db,
                WorkCommand::PreviewHandoff {
                    id: handoff.id,
                    format: "markdown".into()
                }
            )
            .is_err()
        );
        assert!(ledger.task(task.id).unwrap().is_some());
        drop(ledger);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn review_receipt_preserves_older_page_snapshot_and_rejects_changed_proofs() {
        use agenturm_kernel::ObservationStore;
        let directory = scratch("review-proof");
        let db = directory.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let observation = |id: usize| Observation {
            harness_id: ExtensionId::parse("harness.fixture").unwrap(),
            event_id: format!("event-{id:04}"),
            revision: 0,
            identity: ObservationIdentity::session("review-session"),
            observed_at_unix_ns: id as u64,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };
        let rows = (1..=503).map(observation).collect::<Vec<_>>();
        store
            .commit("harness.fixture", None, "seed", &rows)
            .unwrap();
        let mut query = ObservationQuery {
            harness_id: Some("harness.fixture".into()),
            session_id: Some("review-session".into()),
            ..ObservationQuery::latest(crate::INSPECT_PAGE_SIZE)
        };
        let first = store.query_observations(&query).unwrap();
        query.before = first.older_position();
        query.snapshot_id = Some(first.snapshot_id);
        let older = store.query_observations(&query).unwrap();
        assert_eq!(older.observations.len(), 3);
        let pinned = agenturm_ui::PinnedReviewQuery {
            anchor: WorkAnchor {
                source: "harness.fixture".into(),
                session: "review-session".into(),
                worker: None,
                turn: None,
            },
            snapshot_id: older.snapshot_id,
            history_identity: store.workspace_history_identity().unwrap(),
            version: None,
            before: query.before,
            include_reviews: false,
            all_workers: true,
            events: older
                .observations
                .iter()
                .map(|row| agenturm_kernel::ObservationPosition {
                    observed_at_unix_ns: row.observed_at_unix_ns,
                    harness_id: row.harness_id.as_str().into(),
                    event_id: row.event_id.clone(),
                    revision: row.revision,
                    snapshot_id: older.snapshot_id,
                })
                .collect(),
        };
        store
            .commit("harness.fixture", Some("seed"), "new", &[observation(504)])
            .unwrap();
        let text = crate::review_receipt_for_scope(&store, &db, &pinned, "json", None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["observations"].as_array().unwrap().len(), 3);
        assert!(
            text.contains("event-0001")
                && !text.contains("event-0504")
                && !text.contains("event-0503")
        );
        let mut stale = pinned.clone();
        stale.history_identity = "replaced-history".into();
        assert!(crate::review_receipt_for_scope(&store, &db, &stale, "json", None).is_err());
        let mut wrong_page = pinned.clone();
        wrong_page.before = None;
        assert!(crate::review_receipt_for_scope(&store, &db, &wrong_page, "json", None).is_err());
        let mut forged = pinned.clone();
        forged.events[0].revision += 1;
        assert!(crate::review_receipt_for_scope(&store, &db, &forged, "json", None).is_err());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn worker_prompt_scope_reaches_the_exact_scoped_reader() {
        let anchor = WorkAnchor {
            source: "harness.fixture".into(),
            session: "main-session".into(),
            worker: Some("worker-a".into()),
            turn: Some("turn-a".into()),
        };
        let result = prompt_history_with(anchor, None, |owner, cursor| {
            assert_eq!(owner.worker.as_deref(), Some("worker-a"));
            assert!(cursor.is_none());
            Ok(crate::prompt_history::scoped_window(
                vec![crate::content_preview::PromptReference {
                    event_id: "worker-a-event".into(),
                    revision: 0,
                    turn_id: "turn-a".into(),
                    observed_at_unix_ns: 1,
                    snapshot_id: Some(9.into()),
                    history_identity: Some("history".into()),
                }],
                owner.turn.as_deref(),
                cursor,
                crate::prompt_history::DEFAULT_PROMPT_PAGE,
            ))
        });
        let WorkResult::PromptHistory(page) = result else {
            panic!("typed prompt result");
        };
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].anchor.worker.as_deref(), Some("worker-a"));
    }

    #[test]
    fn local_and_broker_prompt_policy_share_exact_turn_cap_cursor_and_stale_checks() {
        let anchor = WorkAnchor {
            source: "harness.fixture".into(),
            session: "session".into(),
            worker: None,
            turn: Some("exact-turn".into()),
        };
        let references = |snapshot: u64| {
            (0..256)
                .map(|index| crate::content_preview::PromptReference {
                    event_id: format!("event-{index:04}"),
                    revision: 0,
                    turn_id: if index < 200 {
                        "exact-turn"
                    } else {
                        "other-turn"
                    }
                    .into(),
                    observed_at_unix_ns: index,
                    snapshot_id: Some(snapshot.into()),
                    history_identity: Some("history".into()),
                })
                .collect()
        };
        let load =
            |snapshot, owner: &WorkAnchor, cursor: Option<&crate::prompt_history::PromptCursor>| {
                Ok(crate::prompt_history::scoped_window(
                    references(snapshot),
                    owner.turn.as_deref(),
                    cursor,
                    100,
                ))
            };
        let WorkResult::PromptHistory(first) =
            prompt_history_with(anchor.clone(), None, |owner, cursor| load(9, owner, cursor))
        else {
            panic!("page");
        };
        assert_eq!(first.entries.len(), 100);
        assert_eq!(first.retained, 200);
        assert!(first.limited);
        assert!(
            first
                .entries
                .iter()
                .all(|row| row.anchor.turn.as_deref() == Some("exact-turn"))
        );
        let before = first.older.unwrap();
        let WorkResult::PromptHistory(second) =
            prompt_history_with(anchor.clone(), Some(before.clone()), |owner, cursor| {
                load(9, owner, cursor)
            })
        else {
            panic!("page");
        };
        assert_eq!(second.entries.len(), 100);
        assert!(second.older.is_none());
        assert!(second.entries.iter().all(|row| {
            !first
                .entries
                .iter()
                .any(|prior| prior.event_id == row.event_id)
        }));
        let WorkResult::PromptHistory(stale) =
            prompt_history_with(anchor.clone(), Some(before), |owner, cursor| {
                load(10, owner, cursor)
            })
        else {
            panic!("page");
        };
        assert!(matches!(
            stale.availability,
            agenturm_ui::PromptAvailability::Stale(_)
        ));
        assert!(stale.entries.is_empty());
        let WorkResult::PromptHistory(denied) = prompt_history_with(anchor, None, |_, _| {
            Err(crate::prompt_history::PromptHistoryError::Denied(
                "Grant revoked".into(),
            ))
        }) else {
            panic!("page");
        };
        assert!(matches!(
            denied.availability,
            agenturm_ui::PromptAvailability::Denied(_)
        ));
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
    fn review_decision_notes_are_bound_to_the_exact_evidence_version() {
        use agenturm_kernel::ObservationStore;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "agenturm-review-notes-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let db = dir.join("history.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let mut identity = ObservationIdentity::session("same-session");
        identity.turn_id = Some("recorded-turn".into());
        let observation = |event: &str, at: u64| Observation {
            harness_id: ExtensionId::parse("harness.codex").unwrap(),
            event_id: event.into(),
            revision: 0,
            identity: identity.clone(),
            observed_at_unix_ns: at,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };
        store
            .commit("harness.codex", None, "one", &[observation("check", 10)])
            .unwrap();
        let catalog = super::super::load_session_catalog(&store, &db, None).unwrap();
        let session = catalog
            .sessions
            .into_iter()
            .find(|row| row.session_id == "same-session")
            .unwrap();
        let WorkResult::ReviewNotes { items, .. } =
            execute(&store, &db, WorkCommand::ReviewNotes(session.clone())).unwrap()
        else {
            panic!("review notes page")
        };
        assert!(items.is_empty());
        let WorkResult::ReviewNotes {
            items,
            session: saved,
        } = execute(
            &store,
            &db,
            WorkCommand::SaveReviewNote {
                session: session.clone(),
                note: "Checks pass on the recorded evidence".into(),
            },
        )
        .unwrap()
        else {
            panic!("saved review notes page")
        };
        assert_eq!(saved.review_version, session.review_version);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].version, session.review_version);
        assert_eq!(items[0].note, "Checks pass on the recorded evidence");

        // A later admitted event changes the receipt version for the session.
        store
            .commit(
                "harness.codex",
                Some("one"),
                "two",
                &[observation("later-check", 11)],
            )
            .unwrap();
        let catalog = super::super::load_session_catalog(&store, &db, None).unwrap();
        let newer = catalog
            .sessions
            .into_iter()
            .find(|row| row.session_id == "same-session")
            .unwrap();
        assert_ne!(newer.review_version, session.review_version);
        let newer_version = newer.review_version;
        let WorkResult::ReviewNotes { items, .. } =
            execute(&store, &db, WorkCommand::ReviewNotes(newer)).unwrap()
        else {
            panic!("review notes page")
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].version, session.review_version);
        assert_ne!(items[0].version, newer_version);
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
