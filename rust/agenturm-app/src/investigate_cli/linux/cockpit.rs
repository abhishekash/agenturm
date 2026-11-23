//! Body-free cockpit results and explicit, grant-fenced source delivery.

use super::*;
use agenturm_ui::{
    InvestigationKind, InvestigationMatch, InvestigationPage, InvestigationRead, PromptDelivery,
    WorkResult,
};

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
    store: &dyn ObservationStore,
    workspace: &Path,
    private: bool,
) -> Result<WorkResult, String> {
    let history = local_history(workspace)?;
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
        snapshot_id: request.snapshot_id,
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
    private_command(
        command,
        &Authority::Local(store, &resolver),
        &local_history(db)?,
    )
}

pub(crate) fn broker(
    command: agenturm_ui::WorkCommand,
    store: &NativeStore,
    workspace: &Path,
) -> Result<WorkResult, String> {
    private_command(
        command,
        &Authority::Broker(store),
        &local_history(workspace)?,
    )
}

fn private_command(
    command: agenturm_ui::WorkCommand,
    authority: &Authority<'_>,
    history: &str,
) -> Result<WorkResult, String> {
    match command {
        agenturm_ui::WorkCommand::Investigate(request) => search(*request, authority, history),
        agenturm_ui::WorkCommand::RevealInvestigation {
            reference,
            delivery,
        } => reveal(&reference, delivery, authority, history),
        _ => Err("Unsupported private investigation request".into()),
    }
}
