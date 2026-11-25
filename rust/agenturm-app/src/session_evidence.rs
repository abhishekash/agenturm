use agenturm_kernel::{
    ExtensionId, MetricRow, Observation, ObservationKind, ObservationScope, PaneModel, Snapshot,
    SpanTokenUsage, TraceProgress, TraceSpan, format_credits, format_reported_cost,
};
use agenturm_ui::{EvidenceTable, EvidenceTables, OperationRow, OperationTables};

mod table;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct SessionPaneIds {
    pub(super) overview: ExtensionId,
    pub(super) activity: ExtensionId,
    pub(super) tables: OperationTables,
    pub(super) evidence: EvidenceTables,
    pub(super) review: ExtensionId,
    pub(super) usage: ExtensionId,
}

pub(super) struct WorkspacePaneIds {
    pub(super) activity: ExtensionId,
    pub(super) tables: OperationTables,
    pub(super) evidence: EvidenceTables,
    pub(super) mcp: ExtensionId,
    pub(super) findings: ExtensionId,
    pub(super) operations: ExtensionId,
    pub(super) usage: ExtensionId,
    pub(super) traces: ExtensionId,
}

#[derive(Clone, Copy)]
enum ViewScope {
    Workspace,
    Session,
}

pub(super) fn add_session_trace_pane(
    snapshot: &mut Snapshot,
    source_id: &str,
    session_id: &str,
) -> (ExtensionId, EvidenceTable) {
    let own_trace_source = source_id.starts_with("otlp.");
    let evidence: Vec<_> = snapshot
        .observations
        .iter()
        .filter(|row| {
            let target = match &row.kind {
                ObservationKind::TraceSpan(span) => span.target_source_id.as_deref(),
                ObservationKind::TraceProgress(progress) => progress.target_source_id.as_deref(),
                ObservationKind::SpanTokenUsage(usage) => usage.target_source_id.as_deref(),
                _ => return false,
            };
            row.identity.session_id.as_deref() == Some(session_id)
                && if own_trace_source {
                    row.harness_id.as_str() == source_id
                } else {
                    row.harness_id.as_str().starts_with("otlp.") && target == Some(source_id)
                }
        })
        .collect();
    let mut rows = vec![metric("Source", source_id), metric("Session", session_id)];
    let (trace_rows, mut table) = trace_presentation(&evidence, ViewScope::Workspace);
    table.context = rows.clone();
    if !own_trace_source {
        table.spec.empty = "No trace evidence explicitly linked to this session.";
    }
    rows.extend(rows_or_empty(
        trace_rows,
        if own_trace_source {
            "No trace evidence recorded for this session"
        } else {
            "No trace evidence explicitly linked to this session"
        },
    ));
    (
        insert_pane(snapshot, "app.session-traces", "Traces", rows),
        table,
    )
}

pub(super) fn add_workspace_panes(snapshot: &mut Snapshot) -> WorkspacePaneIds {
    let mut evidence: Vec<_> = snapshot.observations.iter().collect();
    evidence.sort_unstable_by_key(|&row| observation_order(row));
    let scope = ViewScope::Workspace;
    let activity = rows_or_empty(
        evidence
            .iter()
            .map(|row| activity_row(row, scope))
            .collect(),
        "No recorded activity on this page",
    );
    let (mcp, calls) = mcp_rows(&evidence, scope);
    let mcp = rows_or_empty(mcp, "No recorded MCP tool observations on this page");
    let findings = rows_or_empty(
        evidence
            .iter()
            .filter(|row| matches!(row.kind, ObservationKind::Diagnostic { .. }))
            .map(|row| activity_row(row, scope))
            .collect(),
        "No recorded diagnostics on this page",
    );
    let (operations, groups) = operation_rows(&evidence, scope);
    let operations = rows_or_empty(operations, "No recorded operation identities on this page");
    let usage = usage_rows(&evidence, scope);
    let (traces, trace_table) = trace_presentation(&evidence, scope);
    let traces = rows_or_empty(traces, "No trace evidence on this page");
    let activity_table = table::activity(&evidence);
    let findings_table = table::findings(&evidence);
    let usage_table = table::usage(&evidence);
    let mcp = insert_pane(snapshot, "app.workspace-mcp", "MCP", mcp);
    let operations = insert_pane(
        snapshot,
        "app.workspace-operations",
        "Operations",
        operations,
    );
    let activity = insert_pane(snapshot, "app.workspace-activity", "Activity", activity);
    let findings = insert_pane(snapshot, "app.workspace-findings", "Findings", findings);
    let usage = insert_pane(snapshot, "app.workspace-usage", "Usage", usage);
    let traces = insert_pane(snapshot, "app.workspace-traces", "Traces", traces);
    WorkspacePaneIds {
        tables: [(mcp.clone(), calls), (operations.clone(), groups)].into(),
        evidence: [
            (activity.clone(), activity_table),
            (findings.clone(), findings_table),
            (usage.clone(), usage_table),
            (traces.clone(), trace_table),
        ]
        .into(),
        activity,
        mcp,
        findings,
        operations,
        usage,
        traces,
    }
}

pub(super) fn add_session_panes(
    snapshot: &mut Snapshot,
    source_id: &str,
    session_id: &str,
) -> SessionPaneIds {
    let mut evidence: Vec<_> = snapshot
        .observations
        .iter()
        .filter(|row| {
            row.harness_id.as_str() == source_id
                && row.identity.scope == ObservationScope::Session
                && row.identity.session_id.as_deref() == Some(session_id)
        })
        .collect();
    evidence.sort_unstable_by_key(|&row| observation_order(row));

    let mut overview = vec![metric("Source", source_id), metric("Session", session_id)];
    let turns = evidence
        .iter()
        .filter_map(|row| turn_identity(row))
        .collect::<BTreeSet<_>>();
    overview.push(metric("Recorded turns on page", turns.len().to_string()));
    if let Some(latest) = evidence.last() {
        overview.push(metric("Latest activity on page", timestamp(latest)));
    }
    let latest_lifecycle = evidence.iter().rev().find_map(|row| match &row.kind {
        ObservationKind::SessionStarted => Some((row, "started", None)),
        ObservationKind::SessionEnded => Some((row, "ended", None)),
        ObservationKind::Lifecycle { state, reason_code } => {
            Some((row, state.as_str(), reason_code.as_deref()))
        }
        _ => None,
    });
    if let Some((row, state, reason)) = latest_lifecycle {
        overview.push(metric(
            "Latest recorded state",
            with_turn(row, lifecycle(state, reason)),
        ));
    }
    let turn_endings = recorded_turn_endings(&evidence);
    if let Some((row, state, reason)) = evidence.iter().rev().find_map(|row| turn_end(row)) {
        overview.push(metric(
            "Latest recorded turn end",
            with_turn(row, lifecycle(state, reason)),
        ));
    } else {
        overview.push(metric("Turn end", "No turn end recorded on this page"));
    }
    if snapshot.observations_truncated {
        overview.push(metric("History", "Earlier observations are available"));
    }
    overview.extend(declared_worker_rows(&evidence));

    let activity = rows_or_empty(
        evidence
            .iter()
            .map(|row| activity_row(row, ViewScope::Session))
            .collect(),
        "No observations for this session on this page",
    );
    let review = review_rows(&evidence, &turn_endings);
    let (mcp, calls) = mcp_rows(&evidence, ViewScope::Session);
    let mut tables = OperationTables::new();
    let (operations, groups) = operation_rows(&evidence, ViewScope::Session);
    let usage = usage_rows(&evidence, ViewScope::Session);
    let (traces, trace_table) = trace_presentation(&evidence, ViewScope::Session);
    let mut activity_table = table::activity(&evidence);
    activity_table.spec.empty = "No observations for this session on this page.";
    let usage_table = table::usage(&evidence);
    if !mcp.is_empty() {
        tables.insert(insert_pane(snapshot, "app.session-mcp", "MCP", mcp), calls);
    }
    if !operations.is_empty() {
        tables.insert(
            insert_pane(snapshot, "app.session-operations", "Operations", operations),
            groups,
        );
    }
    let activity = insert_pane(snapshot, "app.session-activity", "Activity", activity);
    let usage = insert_pane(snapshot, "app.session-usage", "Usage", usage);
    let mut evidence = EvidenceTables::from([
        (activity.clone(), activity_table),
        (usage.clone(), usage_table),
    ]);
    if !traces.is_empty() {
        let id = insert_pane(snapshot, "app.session-traces", "Traces", traces);
        evidence.insert(id, trace_table);
    }
    SessionPaneIds {
        tables,
        evidence,
        overview: insert_pane(
            snapshot,
            "app.session-overview",
            "Session overview",
            overview,
        ),
        activity,
        review: insert_pane(snapshot, "app.session-review", "Review", review),
        usage,
    }
}

fn declared_worker_rows(evidence: &[&Observation]) -> Vec<MetricRow> {
    let declarations = evidence
        .iter()
        .filter_map(|row| {
            let ObservationKind::WorkerSpawn(spawn) = &row.kind else {
                return None;
            };
            Some((
                spawn.child_worker_id.as_str(),
                row.identity.worker_id.as_deref(),
                row.identity.turn_id.as_deref()?,
                row.identity.operation_id.as_deref()?,
            ))
        })
        .collect::<BTreeSet<_>>();
    if declarations.is_empty() {
        return Vec::new();
    }
    let distinct_children = declarations
        .iter()
        .map(|(child, _, _, _)| *child)
        .collect::<BTreeSet<_>>()
        .len();
    let mut rows = vec![metric(
        "Declared child workers on page",
        distinct_children.to_string(),
    )];
    rows.extend(
        declarations
            .into_iter()
            .map(|(child, parent, turn, operation)| {
                let parent = parent.unwrap_or("Unreported");
                metric(
                    "Declared child worker",
                    format!(
                        "{child} · parent worker {parent} · turn {turn} · operation {operation}"
                    ),
                )
            }),
    );
    rows
}

pub(super) fn insert_pane(
    snapshot: &mut Snapshot,
    base: &str,
    title: &str,
    rows: Vec<MetricRow>,
) -> ExtensionId {
    let id = std::iter::once(base.to_owned())
        .chain((1usize..).map(|suffix| format!("{base}.{suffix}")))
        .map(|candidate| ExtensionId::parse(candidate).expect("built-in session pane ID is valid"))
        .find(|candidate| !snapshot.panes.contains_key(candidate))
        .expect("snapshot has a finite number of panes");
    snapshot.panes.insert(
        id.clone(),
        PaneModel {
            title: title.into(),
            rows,
        },
    );
    id
}

fn observation_order(row: &Observation) -> (u64, &str, &str, u32) {
    (
        row.observed_at_unix_ns,
        row.harness_id.as_str(),
        row.event_id.as_str(),
        row.revision,
    )
}

type TurnIdentity<'a> = (Option<&'a str>, &'a str);
type TurnEndings<'a> = BTreeMap<TurnIdentity<'a>, (&'a str, Option<&'a str>)>;

fn turn_identity(row: &Observation) -> Option<TurnIdentity<'_>> {
    Some((
        row.identity.worker_id.as_deref(),
        row.identity.turn_id.as_deref()?,
    ))
}

fn turn_end(row: &Observation) -> Option<(&Observation, &str, Option<&str>)> {
    turn_identity(row)?;
    match &row.kind {
        ObservationKind::Lifecycle { state, reason_code }
            if matches!(state.as_str(), "completed" | "interrupted" | "failed") =>
        {
            Some((row, state, reason_code.as_deref()))
        }
        _ => None,
    }
}

fn recorded_turn_endings<'a>(evidence: &[&'a Observation]) -> TurnEndings<'a> {
    evidence
        .iter()
        .filter_map(|row| {
            let (row, state, reason) = turn_end(row)?;
            Some((turn_identity(row)?, (state, reason)))
        })
        .collect()
}

fn review_rows(evidence: &[&Observation], turn_endings: &TurnEndings<'_>) -> Vec<MetricRow> {
    let mut work_results = Vec::new();
    let mut edits = Vec::new();
    let mut checks = Vec::new();
    let mut errors = Vec::new();
    let mut tools = Vec::new();
    let recorded_edits: BTreeSet<_> = evidence
        .iter()
        .filter(|row| {
            matches!(
                row.kind,
                ObservationKind::FileEdit(_) | ObservationKind::FileChange(_)
            )
        })
        .filter_map(|row| operation_occurrence(row, "succeeded"))
        .collect();
    for row in evidence {
        let (group, label) = match &row.kind {
            ObservationKind::PullRequestRaised { .. } => (&mut work_results, "Work result"),
            ObservationKind::FileEdit(_) => (&mut edits, "File edit"),
            ObservationKind::FileChange(_) => (&mut edits, "File change"),
            ObservationKind::Check { .. } => (&mut checks, "Check"),
            ObservationKind::Diagnostic { severity, .. } if severity == "error" => {
                (&mut errors, "Error")
            }
            ObservationKind::Lifecycle { state, .. } if state == "failed" => (&mut errors, "Error"),
            ObservationKind::Tool { category, result }
                if category == "file"
                    && operation_occurrence(row, result)
                        .is_some_and(|operation| recorded_edits.contains(&operation)) =>
            {
                continue;
            }
            ObservationKind::Tool { .. } => (&mut tools, "Tool"),
            _ => continue,
        };
        let (_, details) = describe_observation(&row.kind);
        let mut value = with_turn(row, details);
        if matches!(row.kind, ObservationKind::PullRequestRaised { .. }) {
            let outcome = turn_identity(row)
                .and_then(|turn| turn_endings.get(&turn))
                .map_or("turn end unrecorded on page".into(), |(state, reason)| {
                    lifecycle(state, *reason)
                });
            value.push_str(" · ");
            value.push_str(&outcome);
        }
        group.push(metric(label, with_operation(row, value)));
    }
    work_results.append(&mut edits);
    work_results.append(&mut checks);
    work_results.append(&mut errors);
    work_results.append(&mut tools);
    rows_or_empty(
        work_results,
        "No checks, tools, errors, or work results on this page",
    )
}

fn mcp_rows(evidence: &[&Observation], scope: ViewScope) -> (Vec<MetricRow>, Vec<OperationRow>) {
    let detailed: BTreeSet<_> = evidence
        .iter()
        .filter_map(|row| match &row.kind {
            ObservationKind::McpCall { result, .. } => operation_occurrence(row, result),
            _ => None,
        })
        .collect();
    evidence
        .iter()
        .filter(|row| match &row.kind {
            ObservationKind::McpCall { .. } => true,
            ObservationKind::Tool { category, result } if category == "mcp" => {
                operation_occurrence(row, result)
                    .is_none_or(|occurrence| !detailed.contains(&occurrence))
            }
            _ => false,
        })
        .map(|row| (activity_row(row, scope), operation_summary(row, &[row])))
        .unzip()
}

type OperationOccurrence<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
    u64,
    &'a str,
);

fn operation_occurrence<'a>(
    row: &'a Observation,
    result: &'a str,
) -> Option<OperationOccurrence<'a>> {
    Some((
        row.harness_id.as_str(),
        row.identity.session_id.as_deref(),
        row.identity.worker_id.as_deref(),
        row.identity.turn_id.as_deref(),
        row.identity.operation_id.as_deref()?,
        row.observed_at_unix_ns,
        result,
    ))
}

fn usage_rows(evidence: &[&Observation], scope: ViewScope) -> Vec<MetricRow> {
    rows_or_empty(
        evidence
            .iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    ObservationKind::Usage { .. }
                        | ObservationKind::CreditUsage { .. }
                        | ObservationKind::ReportedCost(_)
                        | ObservationKind::SpanTokenUsage(_)
                )
            })
            .map(|row| activity_row(row, scope))
            .collect(),
        "No reported usage on this page",
    )
}

type ProgressScope<'a> = (Option<&'a str>, Option<&'a str>, Option<&'a str>);

#[derive(Default)]
struct TraceEvidence<'a> {
    spans: Vec<(&'a Observation, &'a TraceSpan)>,
    usage: Vec<(&'a Observation, &'a SpanTokenUsage)>,
    progress: BTreeMap<ProgressScope<'a>, (&'a Observation, &'a TraceProgress)>,
}

fn trace_presentation(
    evidence: &[&Observation],
    scope: ViewScope,
) -> (Vec<MetricRow>, EvidenceTable) {
    let mut traces = BTreeMap::<_, TraceEvidence<'_>>::new();
    for &row in evidence {
        let Some(session) = row.identity.session_id.as_deref() else {
            continue;
        };
        let source = row.harness_id.as_str();
        match &row.kind {
            ObservationKind::TraceSpan(span) => traces
                .entry((source, session, span.trace_id.as_str()))
                .or_default()
                .spans
                .push((row, span)),
            ObservationKind::SpanTokenUsage(usage) => traces
                .entry((source, session, usage.span.trace_id.as_str()))
                .or_default()
                .usage
                .push((row, usage)),
            ObservationKind::TraceProgress(progress) => {
                let owner = (
                    progress.target_source_id.as_deref(),
                    row.identity.turn_id.as_deref(),
                    row.identity.worker_id.as_deref(),
                );
                let recent = traces
                    .entry((source, session, progress.trace_id.as_str()))
                    .or_default()
                    .progress
                    .entry(owner)
                    .or_insert((row, progress));
                if observation_order(row) > observation_order(recent.0) {
                    *recent = (row, progress);
                }
            }
            _ => {}
        }
    }
    let mut groups: Vec<_> = traces
        .into_iter()
        .filter(|(_, evidence)| !evidence.spans.is_empty() || !evidence.progress.is_empty())
        .collect();
    for (_, evidence) in &mut groups {
        evidence
            .spans
            .sort_unstable_by_key(|&(row, span)| (span.started_at_unix_ns, observation_order(row)));
    }
    groups.sort_unstable_by_key(|(identity, evidence)| {
        let earliest = evidence
            .spans
            .first()
            .map(|(_, span)| span.started_at_unix_ns)
            .into_iter()
            .chain(
                evidence
                    .progress
                    .values()
                    .map(|(row, _)| row.observed_at_unix_ns),
            )
            .min()
            .unwrap_or_default();
        (earliest, *identity)
    });

    let mut rows = Vec::new();
    let mut trace_rows = Vec::with_capacity(groups.len());
    for ((source, session, trace), evidence) in groups {
        trace_rows.push(table::trace(source, session, trace, &evidence));
        let label = match scope {
            ViewScope::Workspace => format!("{trace} · {source} · session {session}"),
            ViewScope::Session => trace.to_owned(),
        };
        rows.push(metric("Trace", label));
        for (row, progress) in evidence.progress.into_values() {
            let mut value = with_time(row, with_turn(row, progress_description(progress)));
            if let Some(worker) = &row.identity.worker_id {
                value.push_str(&format!(" · worker {worker}"));
            }
            rows.push(metric("Latest progress", value));
        }
        for (row, span) in evidence.spans {
            let duration = row
                .observed_at_unix_ns
                .saturating_sub(span.started_at_unix_ns);
            rows.push(metric(
                "Span",
                format!(
                    "{} · {} · {}{}",
                    span_description(span),
                    timestamp_at(span.started_at_unix_ns),
                    duration_millis(duration),
                    span.target_source_id
                        .as_ref()
                        .map_or_else(String::new, |target| format!(" · target {target}")),
                ),
            ));
        }
    }
    (rows, table::TRACES.table(trace_rows))
}

fn progress_description(progress: &TraceProgress) -> String {
    let mut value = progress.stage.clone();
    match progress.percent_basis_points {
        Some(points) if points % 100 == 0 => value.push_str(&format!(" · {}%", points / 100)),
        Some(points) => value.push_str(&format!(" · {}.{:02}%", points / 100, points % 100)),
        None => value.push_str(" · percent unavailable"),
    }
    if let Some(target) = &progress.target_source_id {
        value.push_str(&format!(" · target {target}"));
    }
    value
}

fn operation_rows(
    evidence: &[&Observation],
    scope: ViewScope,
) -> (Vec<MetricRow>, Vec<OperationRow>) {
    let mut operations: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for row in evidence {
        if let (Some(session), Some(operation)) = (
            row.identity.session_id.as_deref(),
            row.identity.operation_id.as_deref(),
        ) {
            let key = (
                row.harness_id.as_str(),
                session,
                row.identity.worker_id.as_deref(),
                operation,
            );
            operations.entry(key).or_default().push(*row);
        }
    }

    let mut groups = Vec::new();
    for ((source, session, worker, operation), rows) in operations {
        let turns: BTreeSet<_> = rows
            .iter()
            .filter_map(|row| row.identity.turn_id.as_deref())
            .collect();
        if turns.len() <= 1 {
            groups.push((source, session, worker, operation, rows));
        } else {
            // An operation ID reused across turns cannot bind records with
            // missing turn identities to either reported turn.
            let mut by_turn: BTreeMap<_, Vec<_>> = BTreeMap::new();
            for row in rows {
                by_turn
                    .entry(row.identity.turn_id.as_deref())
                    .or_default()
                    .push(row);
            }
            groups.extend(
                by_turn
                    .into_values()
                    .map(|rows| (source, session, worker, operation, rows)),
            );
        }
    }
    groups.sort_unstable_by_key(|(_, _, _, _, rows)| observation_order(rows[0]));

    let summaries = groups
        .iter()
        .map(|(_, _, _, _, rows)| {
            let latest = rows.last().expect("operation group has observations");
            operation_summary(latest, rows)
        })
        .collect();
    let metrics = groups
        .into_iter()
        .flat_map(|(source, session, worker, operation, rows)| {
            let mut label = worker.map_or_else(
                || operation.to_owned(),
                |worker| format!("{operation} · worker {worker}"),
            );
            if matches!(scope, ViewScope::Workspace) {
                label.push_str(&format!(" · {source} · session {session}"));
            }
            std::iter::once(metric("Operation", label)).chain(rows.into_iter().map(|row| {
                let (label, value) = describe_observation(&row.kind);
                metric(label, with_time(row, with_turn(row, value)))
            }))
        })
        .collect();
    (metrics, summaries)
}

fn operation_summary(row: &Observation, group: &[&Observation]) -> OperationRow {
    let mut result = None;
    let mut name = None;
    let mut server = None;
    let mut duration = None;
    let mut details = vec![metric("Source", row.harness_id.as_str())];
    for (label, value) in [
        ("Session", &row.identity.session_id),
        ("Worker", &row.identity.worker_id),
        ("Operation ID", &row.identity.operation_id),
    ] {
        if let Some(value) = value {
            details.push(metric(label, value));
        }
    }
    for turn in group
        .iter()
        .filter_map(|r| r.identity.turn_id.as_deref())
        .collect::<BTreeSet<_>>()
    {
        details.push(metric("Turn", turn));
    }
    let mut mcp_event = None;
    for observation in group {
        match &observation.kind {
            ObservationKind::McpCall {
                server: host,
                tool,
                method,
                duration_ns,
                result: state,
            } => {
                mcp_event = Some((
                    observation.harness_id.as_str().to_owned(),
                    observation.event_id.clone(),
                    observation.revision,
                ));
                name = Some(tool.as_deref().unwrap_or(method.as_str()));
                server = host.as_deref();
                duration = duration_ns.map(duration_millis);
                result = Some(state.as_str());
            }
            ObservationKind::Tool {
                category,
                result: state,
            } => {
                name.get_or_insert(category.as_str());
                result = Some(state.as_str());
            }
            _ => {}
        }
        let (label, description) = describe_observation(&observation.kind);
        details.push(metric(label, with_time(observation, description)));
    }
    OperationRow {
        time: timestamp(row),
        tool: name
            .unwrap_or_else(|| describe_observation(&row.kind).0)
            .into(),
        server: server.map(str::to_owned),
        result: result.map(str::to_owned),
        latency: duration,
        evidence: details,
        mcp_event,
    }
}

fn activity_row(row: &Observation, scope: ViewScope) -> MetricRow {
    let (label, mut value) = describe_observation(&row.kind);
    if matches!(&row.kind, ObservationKind::ReportedCost(cost) if cost.native_origin.is_some()) {
        value.push_str(&format!(" · revision {}", row.revision));
    }
    let mut value = with_time(row, with_operation(row, with_turn(row, value)));
    if let Some(worker) = row.identity.worker_id.as_deref() {
        value.push_str(&format!(" · worker {worker}"));
    }
    if matches!(scope, ViewScope::Workspace) {
        value.push_str(&format!(" · {}", row.harness_id.as_str()));
        if let Some(session) = row.identity.session_id.as_deref() {
            value.push_str(&format!(" · session {session}"));
        }
    }
    metric(label, value)
}

fn metric(label: &str, value: impl Into<String>) -> MetricRow {
    MetricRow {
        label: label.into(),
        value: value.into(),
    }
}

fn rows_or_empty(rows: Vec<MetricRow>, empty: &str) -> Vec<MetricRow> {
    if rows.is_empty() {
        return vec![metric("Evidence", empty)];
    }
    rows
}

fn with_turn(row: &Observation, mut description: String) -> String {
    if let Some(turn) = row.identity.turn_id.as_deref() {
        description.push_str(" · turn ");
        description.push_str(turn);
    }
    description
}

fn with_operation(row: &Observation, mut description: String) -> String {
    if let Some(operation) = row.identity.operation_id.as_deref() {
        description.push_str(" · operation ");
        description.push_str(operation);
    }
    description
}

fn with_time(row: &Observation, description: String) -> String {
    format!("{} · {description}", timestamp(row))
}

fn timestamp(row: &Observation) -> String {
    let nanos = row.observed_at_unix_ns;
    if nanos == 0 && matches!(row.kind, ObservationKind::Diagnostic { .. }) {
        return "Time unavailable".into();
    }
    timestamp_at(nanos)
}

fn timestamp_at(nanos: u64) -> String {
    #[cfg(target_os = "linux")]
    if let Ok(instant) = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanos)) {
        return format!(
            "{} {:02}:{:02}:{:02}Z",
            instant.date(),
            instant.hour(),
            instant.minute(),
            instant.second()
        );
    }
    format!(
        "Unix {}.{:09}",
        nanos / 1_000_000_000,
        nanos % 1_000_000_000
    )
}

fn duration_millis(duration: u64) -> String {
    let ms = duration / 1_000_000;
    let nanos = duration % 1_000_000;
    if nanos == 0 {
        return format!("{ms} ms");
    }
    let fraction = format!("{nanos:06}");
    format!("{ms}.{} ms", fraction.trim_end_matches('0'))
}

fn span_description(span: &TraceSpan) -> String {
    let status = span.status.as_str();
    match span.parent_span_id.as_deref() {
        Some(parent) => format!("{} · {status} · parent {parent}", span.span_id),
        None => format!("{} · {status}", span.span_id),
    }
}

fn lifecycle(state: &str, reason: Option<&str>) -> String {
    reason.map_or_else(|| state.to_owned(), |reason| format!("{state} · {reason}"))
}

pub(super) fn describe_observation(kind: &ObservationKind) -> (&'static str, String) {
    match kind {
        ObservationKind::SessionStarted => ("Session", "started".into()),
        ObservationKind::SessionEnded => ("Session", "ended".into()),
        ObservationKind::HostSample(sample) => (
            "Host sample",
            format!("{} recorded processes", sample.processes.len()),
        ),
        ObservationKind::Activity { category } => ("Activity", category.clone()),
        ObservationKind::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
        } => {
            let counters = [
                ("input", input_tokens),
                ("output", output_tokens),
                ("cache read", cache_read_tokens),
                ("cache write", cache_write_tokens),
                ("total", total_tokens),
            ];
            let value = counters
                .into_iter()
                .filter_map(|(name, count)| count.map(|count| format!("{name} {count}")))
                .collect::<Vec<_>>()
                .join(" · ");
            ("Usage", value)
        }
        ObservationKind::CreditUsage { nanocredits } => (
            "Credit usage",
            format!("{} credits", format_credits(*nanocredits)),
        ),
        ObservationKind::ReportedCost(cost) => {
            let mut value = format_reported_cost(cost);
            if let Some(origin) = cost.native_origin {
                if origin.rounded {
                    value.insert_str(0, "Approx. ");
                }
                let label = match origin.basis {
                    agenturm_kernel::NativeCostBasis::Estimate => "Cost estimate",
                    agenturm_kernel::NativeCostBasis::ProviderReported => "Provider-reported cost",
                    agenturm_kernel::NativeCostBasis::Unknown => {
                        value.push_str(" · billing basis unknown");
                        "Reported cost"
                    }
                };
                return (label, value);
            }
            if let Some(span) = &cost.span {
                value.push_str(&format!(" · span {}", span.span_id));
            }
            ("Reported cost", value)
        }
        ObservationKind::SpanTokenUsage(usage) => {
            let counters = [
                ("input", usage.input_tokens),
                ("output", usage.output_tokens),
                ("cache read input", usage.cache_read_input_tokens),
                ("cache write input", usage.cache_write_input_tokens),
                ("reasoning output", usage.reasoning_output_tokens),
                ("total", usage.total_tokens),
            ];
            let mut fields: Vec<_> = counters
                .into_iter()
                .filter_map(|(label, value)| value.map(|value| format!("{label} {value}")))
                .collect();
            fields.push(format!("span {}", usage.span.span_id));
            ("Span token usage", fields.join(" · "))
        }
        ObservationKind::Tool { category, result } => ("Tool", format!("{category} · {result}")),
        ObservationKind::FileEdit(edit) => {
            let summary = [
                (edit.added, "added"),
                (edit.updated, "updated"),
                (edit.deleted, "deleted"),
                (edit.moved, "moved"),
            ]
            .into_iter()
            .filter(|(count, _)| *count != 0)
            .map(|(count, action)| format!("{count} {action}"))
            .collect::<Vec<_>>()
            .join(" · ");
            ("File edit", summary)
        }
        ObservationKind::FileChange(change) => (
            "File change",
            format!("{} files · digest {}", change.files, change.digest),
        ),
        ObservationKind::McpCall {
            server,
            tool,
            method,
            duration_ns,
            result,
        } => {
            let mut fields = vec![method.as_str().to_owned(), result.clone()];
            if let Some(server) = server {
                fields.push(format!("server {server}"));
            }
            if let Some(tool) = tool {
                fields.push(format!("tool {tool}"));
            }
            if let Some(duration) = duration_ns {
                fields.push(duration_millis(*duration));
            }
            ("MCP call", fields.join(" · "))
        }
        ObservationKind::TraceProgress(progress) => (
            "Progress",
            format!(
                "{} · trace {}",
                progress_description(progress),
                progress.trace_id
            ),
        ),
        ObservationKind::InternalReview(review) => (
            "Internal review",
            format!("{} · review {}", review.state.as_str(), review.session_id),
        ),
        ObservationKind::NativeRequest(request) => (
            "Native request",
            format!(
                "{} · {}",
                request.request_kind.as_str(),
                request.state.as_str()
            ),
        ),
        ObservationKind::TraceSpanSummary(summary) => (
            "Span classification",
            format!(
                "Agent run {} · reported error {}",
                summary.agent_run, summary.reported_error
            ),
        ),
        ObservationKind::OtlpActivityLog(log) => (
            "Activity log",
            if log.reported_error {
                "Reported error".into()
            } else {
                "Recorded activity".into()
            },
        ),
        ObservationKind::OtlpErrorLog(log) => (
            "Error log",
            format!("Reported severity {}", log.severity_number),
        ),
        ObservationKind::MetricDescriptor(_) => (
            "Metric description",
            "Reported instrument description".into(),
        ),
        ObservationKind::MetricPoint(point) => (
            "Metric point",
            format!(
                "{} · {} · {}",
                point.series.name,
                point.series.kind.label(),
                point.series.unit
            ),
        ),
        ObservationKind::TraceSpan(span) => (
            "Trace span",
            format!("{} · trace {}", span_description(span), span.trace_id),
        ),
        ObservationKind::Lifecycle { state, reason_code } => {
            ("Lifecycle", lifecycle(state, reason_code.as_deref()))
        }
        ObservationKind::Check { category, result } => ("Check", format!("{category} · {result}")),
        ObservationKind::PullRequestRaised { repository, number } => (
            "Pull request raised",
            format!("https://github.com/{repository}/pull/{number}"),
        ),
        ObservationKind::Relationship {
            relationship_kind,
            parent_session_id,
        } => (
            "Session relationship",
            format!(
                "{} · parent {parent_session_id}",
                relationship_kind.as_str()
            ),
        ),
        ObservationKind::WorkerSpawn(spawn) => (
            "Worker declared",
            format!("Child worker {}", spawn.child_worker_id),
        ),
        ObservationKind::WorkerLaunch(launch) => (
            "Worker launched",
            format!("Child worker {}", launch.child_worker_id),
        ),
        ObservationKind::Message { role } => ("Message role", role.clone()),
        ObservationKind::Diagnostic { code, severity } => {
            ("Diagnostic", format!("{code} · {severity}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{McpMethod, ObservationIdentity};

    #[test]
    fn mcp_view_prefers_exact_details_and_retains_other_call_evidence() {
        let coarse = tool(1, "mcp", "failed", Some("turn-1"));
        let mut detailed = coarse.clone();
        detailed.event_id = "mcp-details".into();
        detailed.kind = ObservationKind::McpCall {
            server: Some("github".into()),
            tool: Some("search_code".into()),
            method: McpMethod::ToolsCall,
            duration_ns: Some(1_000_000_005),
            result: "failed".into(),
        };
        let mut other_source = coarse.clone();
        other_source.harness_id = ExtensionId::parse("harness.pi").unwrap();
        let (rows, calls) = mcp_rows(
            &[
                &coarse,
                &other_source,
                &detailed,
                &tool(2, "mcp", "started", None),
            ],
            ViewScope::Workspace,
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(calls[1].tool, "search_code");
        assert_eq!(calls[1].server.as_deref(), Some("github"));
        assert_eq!(rows[0].label, "Tool");
        assert_eq!(rows[1].label, "MCP call");
        assert!(rows[1].value.contains("tools/call · failed"));
        assert!(
            rows[1]
                .value
                .contains("server github · tool search_code · 1000.000005 ms")
        );
        assert!(rows[2].value.contains("mcp · started"));
    }

    fn observation(sequence: u64, kind: ObservationKind, turn: Option<&str>) -> Observation {
        let mut identity = ObservationIdentity::session("session-a");
        identity.turn_id = turn.map(str::to_owned);
        Observation {
            harness_id: ExtensionId::parse("harness.codex").unwrap(),
            event_id: format!("event-{sequence}"),
            revision: 0,
            identity,
            observed_at_unix_ns: sequence * 1_000_000_000,
            kind,
        }
    }

    #[test]
    fn trace_pane_shows_latest_reported_progress_per_worker_without_a_span() {
        let progress = |sequence, worker: &str, stage: &str, percent_basis_points| {
            let mut row = observation(
                sequence,
                ObservationKind::TraceProgress(TraceProgress {
                    trace_id: "11".repeat(16),
                    target_source_id: Some("harness.codex".into()),
                    stage: stage.into(),
                    percent_basis_points,
                }),
                Some("turn-1"),
            );
            row.harness_id = ExtensionId::parse("otlp.fixture").unwrap();
            row.identity.worker_id = Some(worker.into());
            row
        };
        let old = progress(1, "worker-one", "first-stage", Some(0));
        let latest = progress(3, "worker-one", "new-stage", None);
        let sibling = progress(2, "worker-two", "sibling-stage", Some(0));
        let (rows, _) = trace_presentation(&[&latest, &old, &sibling], ViewScope::Workspace);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].label, "Trace");
        assert_eq!(rows[1].label, "Latest progress");
        assert!(rows[1].value.contains("new-stage · percent unavailable"));
        assert!(rows[1].value.contains("worker worker-one"));
        assert!(rows[2].value.contains("sibling-stage · 0%"));
        assert!(rows[2].value.contains("worker worker-two"));
        assert!(
            activity_row(&old, ViewScope::Session)
                .value
                .contains("first-stage · 0%")
        );
    }

    #[test]
    fn trace_table_reports_only_an_exact_root_spans_recorded_tokens() {
        use agenturm_kernel::{TraceSpanIdentity, TraceStatus};
        let trace_id = "11".repeat(16);
        let span_id = "22".repeat(8);
        let root = observation(
            1,
            ObservationKind::TraceSpan(TraceSpan {
                trace_id: trace_id.clone(),
                span_id: span_id.clone(),
                parent_span_id: None,
                target_source_id: None,
                target_operation_id: None,
                started_at_unix_ns: 1,
                status: TraceStatus::Ok,
            }),
            Some("root-turn"),
        );
        let usage = |sequence, turn, input| {
            observation(
                sequence,
                ObservationKind::SpanTokenUsage(SpanTokenUsage {
                    span: TraceSpanIdentity {
                        trace_id: trace_id.clone(),
                        span_id: span_id.clone(),
                    },
                    target_source_id: None,
                    input_tokens: Some(input),
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_write_input_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: None,
                }),
                turn,
            )
        };
        let different_turn = usage(2, Some("sibling-turn"), 99);
        let matching = usage(3, Some("root-turn"), 0);
        let mut different_source = matching.clone();
        different_source.harness_id = ExtensionId::parse("otlp.sibling").unwrap();
        let (_, table) = trace_presentation(
            &[&root, &matching, &different_turn, &different_source],
            ViewScope::Workspace,
        );
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].cells, ["ok", &trace_id, "1", "0 / —"]);
        let (_, table) = trace_presentation(&[&root, &different_turn], ViewScope::Workspace);
        assert_eq!(table.rows[0].cells[3], "Unavailable");
        let duplicate = usage(4, Some("root-turn"), 9999);
        let (_, table) = trace_presentation(&[&root, &matching, &duplicate], ViewScope::Workspace);
        assert_eq!(table.rows[0].cells[3], "Unavailable");
    }

    fn tool(sequence: u64, category: &str, result: &str, turn: Option<&str>) -> Observation {
        let mut row = observation(
            sequence,
            ObservationKind::Tool {
                category: category.into(),
                result: result.into(),
            },
            turn,
        );
        row.identity.operation_id = Some("call-1".into());
        row
    }

    fn pane<'a>(snapshot: &'a Snapshot, name: &str) -> &'a PaneModel {
        &snapshot.panes[&ExtensionId::parse(format!("app.session-{name}")).unwrap()]
    }

    #[test]
    fn declared_worker_links_keep_exact_parent_scope_and_page_coverage() {
        let source = ExtensionId::parse("harness.claude.worker-fixture").unwrap();
        let declaration = |time, child: &str, parent: Option<&str>, turn: &str, op: &str| {
            let mut row = observation(
                time,
                ObservationKind::WorkerSpawn(agenturm_kernel::WorkerSpawn {
                    child_worker_id: child.into(),
                }),
                Some(turn),
            );
            row.harness_id = source.clone();
            row.identity.worker_id = parent.map(str::to_owned);
            row.identity.operation_id = Some(op.into());
            row
        };
        let mut other_session = declaration(5, "outside-session", None, "turn-a", "call-a");
        other_session.identity.session_id = Some("session-b".into());
        let mut other_source = declaration(6, "outside-source", None, "turn-a", "call-a");
        other_source.harness_id = ExtensionId::parse("harness.claude.sibling").unwrap();
        let mut snapshot = Snapshot {
            observations: vec![
                declaration(1, "child-a", Some("parent-a"), "turn-a", "call-a"),
                declaration(2, "child-a", Some("parent-a"), "turn-a", "call-a"),
                declaration(3, "child-a", Some("parent-b"), "turn-b", "call-b"),
                declaration(4, "child-b", None, "turn-c", "call-c"),
                other_session,
                other_source,
            ],
            observations_truncated: true,
            ..Snapshot::default()
        };
        let panes = add_session_panes(&mut snapshot, source.as_str(), "session-a");
        let overview = &snapshot.panes[&panes.overview].rows;
        assert!(overview.contains(&metric("Declared child workers on page", "2")));
        assert!(overview.contains(&metric("History", "Earlier observations are available")));
        let links = overview
            .iter()
            .filter(|row| row.label == "Declared child worker")
            .map(|row| row.value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            links,
            [
                "child-a · parent worker parent-a · turn turn-a · operation call-a",
                "child-a · parent worker parent-b · turn turn-b · operation call-b",
                "child-b · parent worker Unreported · turn turn-c · operation call-c",
            ]
        );
        let activity = &panes.evidence[&panes.activity].rows;
        assert_eq!(activity.len(), 4);
        assert_eq!(&activity[0].cells[2..], &["declared", "child-a"]);
        for (label, value) in [
            ("Declared child worker", "child-a"),
            ("Source", source.as_str()),
            ("Session", "session-a"),
            ("Parent worker", "parent-a"),
            ("Turn", "turn-a"),
            ("Operation", "call-a"),
            ("Event", "event-1"),
            ("Revision", "0"),
        ] {
            assert!(activity[0].details.contains(&metric(label, value)));
        }
        assert!(
            activity[3]
                .details
                .contains(&metric("Parent worker", "Unreported"))
        );
    }

    #[test]
    fn activity_is_chronological_and_review_keeps_work_result_and_recorded_turn_end() {
        let lifecycle = |time, turn, state: &str, reason: Option<&str>| {
            observation(
                time,
                ObservationKind::Lifecycle {
                    state: state.into(),
                    reason_code: reason.map(str::to_owned),
                },
                Some(turn),
            )
        };
        let mut raised = observation(
            2,
            ObservationKind::PullRequestRaised {
                repository: "owner/repo".into(),
                number: 42,
            },
            Some("turn-1"),
        );
        raised.identity.operation_id = Some("pr-call".into());
        let mut edit = observation(
            10,
            ObservationKind::FileEdit(agenturm_kernel::FileEdit {
                added: 1,
                updated: 2,
                deleted: 0,
                moved: 0,
            }),
            Some("turn-2"),
        );
        edit.identity.operation_id = Some("edit-call".into());
        let mut other_edit = edit.clone();
        other_edit.identity.session_id = Some("session-b".into());
        let mut other_session = lifecycle(7, "turn-1", "completed", Some("other"));
        other_session.identity.session_id = Some("session-b".into());
        let mut other_source = lifecycle(8, "turn-1", "completed", Some("another"));
        other_source.harness_id = ExtensionId::parse("harness.pi").unwrap();
        let mut same_turn_different_worker = lifecycle(9, "turn-1", "completed", Some("worker"));
        same_turn_different_worker.identity.worker_id = Some("worker-1".into());
        let mut snapshot = Snapshot {
            observations: vec![
                other_session,
                lifecycle(5, "turn-2", "running", None),
                lifecycle(3, "turn-1", "interrupted", Some("codex_task_aborted")),
                raised,
                tool(1, "shell", "started", Some("turn-1")),
                observation(
                    4,
                    ObservationKind::Check {
                        category: "lint".into(),
                        result: "failed".into(),
                    },
                    Some("turn-1"),
                ),
                other_source,
                same_turn_different_worker,
                edit,
                other_edit,
                observation(
                    6,
                    ObservationKind::Diagnostic {
                        code: "kiro_error".into(),
                        severity: "error".into(),
                    },
                    Some("turn-2"),
                ),
            ],
            observations_truncated: true,
            ..Snapshot::default()
        };
        add_session_panes(&mut snapshot, "harness.codex", "session-a");

        let activity = &pane(&snapshot, "activity").rows;
        assert_eq!(
            activity
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Tool",
                "Pull request raised",
                "Lifecycle",
                "Check",
                "Lifecycle",
                "Diagnostic",
                "Lifecycle",
                "File edit"
            ]
        );
        assert!(
            activity
                .iter()
                .all(|row| !row.value.contains("other") && !row.value.contains("another"))
        );
        let review = &pane(&snapshot, "review").rows;
        assert_eq!(
            review
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["Work result", "File edit", "Check", "Error", "Tool"]
        );
        for expected in [
            "https://github.com/owner/repo/pull/42",
            "turn-1",
            "interrupted",
            "codex_task_aborted",
        ] {
            assert!(review[0].value.contains(expected));
        }
        for expected in ["1 added", "2 updated", "turn-2", "edit-call"] {
            assert!(review[1].value.contains(expected));
        }
        assert_eq!(
            pane(&snapshot, "overview").rows.last().unwrap().label,
            "History"
        );
    }

    #[test]
    fn mcp_and_operation_panes_require_exact_durable_evidence() {
        let mut snapshot = Snapshot {
            observations: vec![
                tool(1, "mcp", "started", Some("turn-1")),
                tool(2, "mcp", "failed", None),
                tool(3, "shell", "started", Some("turn-2")),
            ],
            ..Snapshot::default()
        };
        snapshot.observations[2].identity.operation_id = Some("shell-call".into());
        add_session_panes(&mut snapshot, "harness.codex", "session-a");

        let mcp = &pane(&snapshot, "mcp").rows;
        assert_eq!(mcp.len(), 2);
        assert!(mcp.iter().all(|row| row.value.contains("operation call-1")));
        let operations = &pane(&snapshot, "operations").rows;
        assert_eq!(
            operations
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["Operation", "Tool", "Tool", "Operation", "Tool"]
        );
        assert_eq!(operations[0].value, "call-1");
        assert!(operations[1].value.contains("turn-1"));
        assert!(!operations[2].value.contains("turn-1"));

        let mut ambiguous = snapshot.observations[..2].to_vec();
        ambiguous.push(tool(3, "shell", "started", Some("turn-2")));
        let mut snapshot = Snapshot {
            observations: ambiguous,
            ..Snapshot::default()
        };
        add_session_panes(&mut snapshot, "harness.codex", "session-a");
        assert_eq!(
            pane(&snapshot, "operations")
                .rows
                .iter()
                .filter(|row| row.label == "Operation")
                .count(),
            3
        );

        let mut empty = Snapshot::default();
        add_session_panes(&mut empty, "harness.codex", "session-a");
        assert_eq!(empty.panes.len(), 4);
        assert_eq!(pane(&empty, "overview").rows[1].value, "session-a");
        assert!(
            pane(&empty, "activity").rows[0]
                .value
                .starts_with("No observations")
        );
    }

    #[test]
    fn session_panes_keep_existing_extension_panes_under_colliding_ids() {
        let extension_id = ExtensionId::parse("app.session-overview").unwrap();
        let extension = PaneModel {
            title: "External pane".into(),
            rows: Vec::new(),
        };
        let mut snapshot = Snapshot::default();
        snapshot
            .panes
            .insert(extension_id.clone(), extension.clone());
        let panes = add_session_panes(&mut snapshot, "harness.codex", "session-a");
        assert_ne!(panes.overview, extension_id);
        assert_eq!(snapshot.panes[&extension_id], extension);
        assert_eq!(snapshot.panes[&panes.overview].title, "Session overview");
        assert_eq!(snapshot.panes[&panes.activity].title, "Activity");
        assert_eq!(snapshot.panes[&panes.review].title, "Review");
        assert_eq!(snapshot.panes[&panes.usage].title, "Usage");
    }

    #[test]
    fn workspace_panes_scope_operations_and_keep_currency_credit_and_token_units_separate() {
        let mut elsewhere = tool(2, "mcp", "failed", Some("turn-1"));
        elsewhere.harness_id = ExtensionId::parse("harness.pi").unwrap();
        let mut other_session = tool(3, "mcp", "succeeded", Some("turn-1"));
        other_session.identity.session_id = Some("session-b".into());
        let mut gap = observation(
            4,
            ObservationKind::Diagnostic {
                code: "possible_source_gap".into(),
                severity: "warning".into(),
            },
            None,
        );
        gap.identity = ObservationIdentity::source();
        let inputs = observation(
            5,
            ObservationKind::Usage {
                input_tokens: Some(0),
                output_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_tokens: None,
            },
            Some("turn-1"),
        );
        let credits = observation(6, ObservationKind::CreditUsage { nanocredits: 123 }, None);
        let usd = observation(
            7,
            ObservationKind::ReportedCost(agenturm_kernel::ReportedCost {
                currency: "USD".into(),
                nanounits: 0,
                span: None,
                native_origin: None,
            }),
            None,
        );
        let eur = observation(
            8,
            ObservationKind::ReportedCost(agenturm_kernel::ReportedCost {
                currency: "EUR".into(),
                nanounits: 1_234_567_890,
                span: None,
                native_origin: None,
            }),
            None,
        );
        let mut snapshot = Snapshot {
            observations: vec![
                eur,
                usd,
                credits,
                gap,
                other_session,
                elsewhere,
                tool(1, "shell", "started", Some("turn-1")),
                inputs,
            ],
            ..Snapshot::default()
        };
        let panes = add_workspace_panes(&mut snapshot);
        assert_eq!(snapshot.panes[&panes.activity].rows.len(), 8);
        assert_eq!(snapshot.panes[&panes.mcp].rows.len(), 2);
        let findings = &snapshot.panes[&panes.findings].rows;
        assert_eq!(findings.len(), 1);
        assert!(findings[0].value.contains("possible_source_gap"));
        let groups: Vec<_> = snapshot.panes[&panes.operations]
            .rows
            .iter()
            .filter(|row| row.label == "Operation")
            .map(|row| row.value.as_str())
            .collect();
        assert_eq!(
            groups,
            [
                "call-1 · harness.codex · session session-a",
                "call-1 · harness.pi · session session-a",
                "call-1 · harness.codex · session session-b",
            ]
        );
        let table = &panes.evidence[&panes.usage];
        assert_eq!(
            table
                .rows
                .iter()
                .map(|row| row.cells[3].as_str())
                .collect::<Vec<_>>(),
            ["tokens", "credits", "USD", "EUR"]
        );
        assert_eq!(table.rows[0].cells[2], "input 0");
        assert_eq!(table.rows[2].cells[2], "0 USD");
        let diagnostic = &panes.evidence[&panes.findings].rows[0];
        assert_eq!(&diagnostic.cells[..2], ["warning", "possible_source_gap"]);
        assert!(
            diagnostic
                .details
                .iter()
                .any(|row| row == &metric("Event", "event-4"))
        );
        let usage = &snapshot.panes[&panes.usage].rows;
        assert_eq!(usage.len(), 4);
        assert!(usage[0].value.contains("input 0 · turn turn-1"));
        assert!(!usage[0].value.contains("output"));
        assert!(usage[1].value.contains("0.000000123 credits"));
        assert!(!usage[1].value.contains("turn "));
        assert_eq!(usage[2].label, "Reported cost");
        assert!(usage[2].value.contains(" · 0 USD · "));
        assert!(usage[3].value.contains(" · 1.23456789 EUR · "));
    }
}
