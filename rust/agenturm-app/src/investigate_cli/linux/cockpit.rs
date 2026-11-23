//! Body-free cockpit results and explicit, grant-fenced source delivery.

use super::*;
use agenturm_ui::{
    InvestigationKind, InvestigationMatch, InvestigationPage, InvestigationRead, PromptDelivery,
    WorkResult,
};

const HISTORY_CHANGED: &str =
    "Investigation history changed. Reopen the cockpit and select fresh evidence.";

fn bind_history(workspace: &Path, identity: Option<&str>) -> Result<String, String> {
    hash((
        "agenturm-investigation-cockpit/1",
        local_history(workspace)?,
        identity,
    ))
}

fn match_row(
    row: &Observation,
    position: ObservationPosition,
    label: &'static str,
    source_reference: Option<String>,
) -> InvestigationMatch {
    InvestigationMatch {
        position,
        session: row.identity.session_id.clone(),
        turn: row.identity.turn_id.clone(),
        worker: row.identity.worker_id.clone(),
        label,
        source_reference,
    }
}

pub(crate) fn failures(
    request: &InvestigationRead,
    store: &impl crate::HistoryReader,
    workspace: &Path,
) -> Result<WorkResult, String> {
    let identity = store.workspace_history_identity();
    if request.history_identity.is_some() && request.history_identity != identity {
        return Err(HISTORY_CHANGED.into());
    }
    let history = bind_history(workspace, identity.as_deref())?;
    let private = identity.is_some() && store.allows_private_capture();
    let search = failures::FailureSearch::for_cockpit(request)?;
    let mut rows = Vec::new();
    let coverage = failures::scan(&search, store, &history, |candidate| {
        let reference = if private {
            candidate.encoded_reference(&history)?
        } else {
            None
        };
        rows.push(match_row(
            candidate.row,
            position(candidate.row, candidate.snapshot_id),
            candidate.label(),
            reference,
        ));
        Ok(())
    })?;
    Ok(WorkResult::Investigation(Box::new(InvestigationPage {
        rows,
        history_identity: identity,
        snapshot_id: coverage.snapshot_id,
        inspected: coverage.inspected,
        verified: None,
        unavailable: 0,
        next_cursor: coverage.next_cursor,
    })))
}

fn search(
    request: InvestigationRead,
    authority: &Authority<'_>,
    history: &str,
) -> Result<WorkResult, String> {
    let (kind, failed, label) = match request.kind {
        InvestigationKind::Instruction => (
            EvidenceKind::Instruction,
            false,
            "Verified instruction match",
        ),
        InvestigationKind::Path => (EvidenceKind::Path, false, "Verified path match"),
        InvestigationKind::Command => (EvidenceKind::Command, false, "Verified command match"),
        InvestigationKind::FailedCommand => (
            EvidenceKind::Command,
            true,
            "Confirmed failed command match",
        ),
        InvestigationKind::Failures => return Err("Choose a private investigation mode".into()),
    };
    validate_query(kind, &request.query)?;
    let selected = request
        .owner
        .ok_or("Select a session or turn before searching protected evidence")?;
    let snapshot_id = request.snapshot_id.ok_or(HISTORY_CHANGED)?;
    if !id(&selected.session)
        || selected.turn.as_deref().is_some_and(|turn| !id(turn))
        || selected
            .worker
            .as_deref()
            .is_some_and(|worker| !id(worker) || selected.turn.is_none())
    {
        return Err("Invalid investigation session, turn, or worker scope".into());
    }
    let source_id =
        ExtensionId::parse(&selected.source).map_err(|_| "Invalid investigation source")?;
    let cursor: Option<SearchCursor> = request.cursor.as_deref().map(token).transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.version != 1 || cursor.before.harness_id != source_id.as_str())
    {
        return Err("Invalid investigation cursor".into());
    }
    let search = Search {
        kind,
        failed,
        query: request.query,
        scope: Scope {
            source_id,
            session_id: selected.session,
            turn_id: selected.turn,
            worker_id: selected.worker,
        },
        limit: COCKPIT_PAGE_LIMIT,
        cursor,
        snapshot_id: Some(snapshot_id),
    };
    let mut rows = Vec::new();
    let coverage = scan_search(&search, authority, history, |row, target, proof| {
        let exact = bind_verified_reference(target, &proof, failed);
        if !exact.valid() {
            return Err("Invalid verified investigation reference".into());
        }
        rows.push(match_row(
            row,
            exact.position.clone(),
            label,
            Some(encoded(&exact)?),
        ));
        Ok(())
    })?;
    Ok(WorkResult::Investigation(Box::new(InvestigationPage {
        rows,
        history_identity: request.history_identity,
        snapshot_id: coverage.snapshot_id,
        inspected: coverage.examined,
        verified: Some(coverage.verified),
        unavailable: coverage.unavailable,
        next_cursor: coverage.next,
    })))
}

fn reveal(
    raw: &str,
    mut delivery: PromptDelivery,
    authority: &Authority<'_>,
    history: &str,
) -> Result<WorkResult, String> {
    let reference: SourceReference = token(raw)?;
    if !reference.valid() {
        return Err("Invalid investigation source reference".into());
    }
    verify_reference(&reference, authority, history, |proof, _| {
        let text = match proof {
            Proven::Prompt(prompt) => prompt.text.clone(),
            Proven::Edit(edit) => serde_json::to_string_pretty(&edit.changes)
                .map_err(|_| "Cannot format the verified path evidence")?,
            Proven::Command(command) => serde_json::to_string_pretty(&command.recorded_invocation)
                .map_err(|_| "Cannot format the verified command evidence")?,
        };
        serde_json::to_writer(&mut delivery, &text).map_err(|_| "Investigation display closed")?;
        delivery
            .flush()
            .map_err(|_| "Investigation display was not confirmed".to_owned())
    })?;
    Ok(WorkResult::Status(String::new()))
}

pub(crate) fn local(
    command: agenturm_ui::WorkCommand,
    db: &Path,
    writer: Option<&Store>,
    config: &Path,
) -> Result<WorkResult, String> {
    let resolver = crate::content_preview::source_resolver(config);
    let opened;
    let store = if let Some(writer) = writer {
        writer
    } else {
        (opened, _) = crate::content_preview::open_source_authority(db, config)?;
        &opened
    };
    store.verify_content_writer().map_err(|_| HISTORY_CHANGED)?;
    let identity = store.history_identity().map_err(|_| HISTORY_CHANGED)?;
    private_command(command, &Authority::Local(store, &resolver), db, &identity)
}

pub(crate) fn broker(
    command: agenturm_ui::WorkCommand,
    store: &NativeStore,
    workspace: &Path,
) -> Result<WorkResult, String> {
    let identity = store
        .read_binding()
        .and_then(agenturm_storage_service::logical_read_identity)
        .ok_or(HISTORY_CHANGED)?;
    private_command(command, &Authority::Broker(store), workspace, identity)
}

fn private_command(
    command: agenturm_ui::WorkCommand,
    authority: &Authority<'_>,
    workspace: &Path,
    actual_identity: &str,
) -> Result<WorkResult, String> {
    let expected = match &command {
        agenturm_ui::WorkCommand::Investigate(request) => request.history_identity.as_deref(),
        agenturm_ui::WorkCommand::RevealInvestigation {
            history_identity, ..
        } => history_identity.as_deref(),
        _ => return Err("Unsupported private investigation request".into()),
    };
    if expected != Some(actual_identity) {
        return Err(HISTORY_CHANGED.into());
    }
    let history = bind_history(workspace, Some(actual_identity))?;
    match command {
        agenturm_ui::WorkCommand::Investigate(request) => search(*request, authority, &history),
        agenturm_ui::WorkCommand::RevealInvestigation {
            reference,
            delivery,
            ..
        } => reveal(&reference, delivery, authority, &history),
        _ => Err("Unsupported private investigation request".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ObservationIdentity;
    use agenturm_ui::{WorkAnchor, WorkCommand};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn private_search_and_metadata_references_cannot_rebind_to_a_replacement_history() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-investigate-history-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let original = directory.join("history.sqlite");
        let replacement = directory.join("replacement.sqlite");
        let mut original_writer = Store::open_collector(&original).unwrap();
        let mut replacement_writer = Store::open_collector(&replacement).unwrap();
        let source = ExtensionId::parse("harness.codex").unwrap();
        let mut identity = ObservationIdentity::session("shared-session");
        identity.turn_id = Some("turn".into());
        identity.operation_id = Some("call".into());
        let row = Observation {
            harness_id: source.clone(),
            event_id: "codex:shared-session:turn:call:tool".into(),
            identity,
            revision: 0,
            observed_at_unix_ns: 100,
            kind: ObservationKind::Tool {
                category: "shell".into(),
                result: "failed".into(),
            },
        };
        for writer in [&mut original_writer, &mut replacement_writer] {
            writer
                .commit(source.as_str(), None, "receipt", std::slice::from_ref(&row))
                .unwrap();
        }
        let original_identity = original_writer.history_identity().unwrap();
        let replacement_identity = replacement_writer.history_identity().unwrap();
        assert_ne!(original_identity, replacement_identity);
        let snapshot = |store: &Store| {
            store
                .query_observations(&ObservationQuery::latest(1))
                .unwrap()
                .snapshot_id
        };
        let selected = snapshot(&original_writer);
        assert_eq!(selected, snapshot(&replacement_writer));
        let request = |kind| InvestigationRead {
            kind,
            query: "fixture".into(),
            owner: Some(WorkAnchor {
                source: source.as_str().into(),
                session: "shared-session".into(),
                turn: None,
                worker: None,
            }),
            history_identity: Some(original_identity.clone()),
            snapshot_id: Some(selected),
            cursor: None,
        };

        let open = |store: &Store| {
            local(
                WorkCommand::Investigate(Box::new(request(InvestigationKind::Instruction))),
                &original,
                Some(store),
                &directory.join("sources.json"),
            )
        };
        assert!(matches!(
            open(&original_writer),
            Ok(WorkResult::Investigation(_))
        ));
        assert!(matches!(open(&replacement_writer), Err(error) if error == HISTORY_CHANGED));
        drop(replacement_writer);
        std::fs::rename(&replacement, &original).unwrap();
        assert!(matches!(open(&original_writer), Err(error) if error == HISTORY_CHANGED));
        let WorkResult::Investigation(page) = failures(
            &request(InvestigationKind::Failures),
            &original_writer,
            &original,
        )
        .unwrap() else {
            panic!("expected cached failure metadata");
        };
        let reference: SourceReference =
            token(page.rows[0].source_reference.as_deref().unwrap()).unwrap();
        assert_eq!(reference.version, 2);
        assert_eq!(reference.position.snapshot_id, selected);
        assert_eq!(
            reference.history_sha256,
            bind_history(&original, Some(&original_identity)).unwrap()
        );
        assert_ne!(
            reference.history_sha256,
            bind_history(&original, Some(&replacement_identity)).unwrap()
        );
        drop(original_writer);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
