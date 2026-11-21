use agenturm_kernel::{
    ExtensionId, MetricRow, Observation, ObservationKind, ObservationScope, PaneModel, Snapshot,
    format_credits,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct SessionPaneIds {
    pub(super) overview: ExtensionId,
    pub(super) activity: ExtensionId,
    pub(super) review: ExtensionId,
    pub(super) usage: ExtensionId,
}

pub(super) struct WorkspacePaneIds {
    pub(super) activity: ExtensionId,
    pub(super) mcp: ExtensionId,
    pub(super) findings: ExtensionId,
    pub(super) operations: ExtensionId,
    pub(super) usage: ExtensionId,
}

#[derive(Clone, Copy)]
enum ViewScope {
    Workspace,
    Session,
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
    let mcp = rows_or_empty(
        mcp_rows(&evidence, scope),
        "No recorded MCP tool observations on this page",
    );
    let findings = rows_or_empty(
        evidence
            .iter()
            .filter(|row| matches!(row.kind, ObservationKind::Diagnostic { .. }))
            .map(|row| activity_row(row, scope))
            .collect(),
        "No recorded diagnostics on this page",
    );
    let operations = rows_or_empty(
        operation_rows(&evidence, scope),
        "No recorded operation identities on this page",
    );
    let usage = usage_rows(&evidence, scope);
    WorkspacePaneIds {
        activity: insert_pane(snapshot, "app.workspace-activity", "Activity", activity),
        mcp: insert_pane(snapshot, "app.workspace-mcp", "MCP", mcp),
        findings: insert_pane(snapshot, "app.workspace-findings", "Findings", findings),
        operations: insert_pane(
            snapshot,
            "app.workspace-operations",
            "Operations",
            operations,
        ),
        usage: insert_pane(snapshot, "app.workspace-usage", "Usage", usage),
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

    let activity = rows_or_empty(
        evidence
            .iter()
            .map(|row| activity_row(row, ViewScope::Session))
            .collect(),
        "No observations for this session on this page",
    );
    let review = review_rows(&evidence, &turn_endings);
    let mcp = mcp_rows(&evidence, ViewScope::Session);
    let operations = operation_rows(&evidence, ViewScope::Session);
    let usage = usage_rows(&evidence, ViewScope::Session);
    if !mcp.is_empty() {
        insert_pane(snapshot, "app.session-mcp", "MCP", mcp);
    }
    if !operations.is_empty() {
        insert_pane(snapshot, "app.session-operations", "Operations", operations);
    }
    SessionPaneIds {
        overview: insert_pane(
            snapshot,
            "app.session-overview",
            "Session overview",
            overview,
        ),
        activity: insert_pane(snapshot, "app.session-activity", "Activity", activity),
        review: insert_pane(snapshot, "app.session-review", "Review", review),
        usage: insert_pane(snapshot, "app.session-usage", "Usage", usage),
    }
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
    let mut checks = Vec::new();
    let mut errors = Vec::new();
    let mut tools = Vec::new();
    for row in evidence {
        let (group, label) = match &row.kind {
            ObservationKind::PullRequestRaised { .. } => (&mut work_results, "Work result"),
            ObservationKind::Check { .. } => (&mut checks, "Check"),
            ObservationKind::Diagnostic { severity, .. } if severity == "error" => {
                (&mut errors, "Error")
            }
            ObservationKind::Lifecycle { state, .. } if state == "failed" => (&mut errors, "Error"),
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
    work_results.append(&mut checks);
    work_results.append(&mut errors);
    work_results.append(&mut tools);
    rows_or_empty(
        work_results,
        "No checks, tools, errors, or work results on this page",
    )
}

fn mcp_rows(evidence: &[&Observation], scope: ViewScope) -> Vec<MetricRow> {
    evidence
        .iter()
        .filter(
            |row| matches!(&row.kind, ObservationKind::Tool { category, .. } if category == "mcp"),
        )
        .map(|row| activity_row(row, scope))
        .collect()
}

fn usage_rows(evidence: &[&Observation], scope: ViewScope) -> Vec<MetricRow> {
    rows_or_empty(
        evidence
            .iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    ObservationKind::Usage { .. } | ObservationKind::CreditUsage { .. }
                )
            })
            .map(|row| activity_row(row, scope))
            .collect(),
        "No reported token or credit usage on this page",
    )
}

fn operation_rows(evidence: &[&Observation], scope: ViewScope) -> Vec<MetricRow> {
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

    groups
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
        .collect()
}

fn activity_row(row: &Observation, scope: ViewScope) -> MetricRow {
    let (label, value) = describe_observation(&row.kind);
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

fn lifecycle(state: &str, reason: Option<&str>) -> String {
    reason.map_or_else(|| state.to_owned(), |reason| format!("{state} · {reason}"))
}

pub(super) fn describe_observation(kind: &ObservationKind) -> (&'static str, String) {
    match kind {
        ObservationKind::SessionStarted => ("Session", "started".into()),
        ObservationKind::SessionEnded => ("Session", "ended".into()),
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
        ObservationKind::Tool { category, result } => ("Tool", format!("{category} · {result}")),
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
        ObservationKind::Message { role } => ("Message role", role.clone()),
        ObservationKind::Diagnostic { code, severity } => {
            ("Diagnostic", format!("{code} · {severity}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ObservationIdentity;

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
                "Lifecycle"
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
            ["Work result", "Check", "Error", "Tool"]
        );
        for expected in [
            "https://github.com/owner/repo/pull/42",
            "turn-1",
            "interrupted",
            "codex_task_aborted",
        ] {
            assert!(review[0].value.contains(expected));
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
    fn workspace_panes_scope_operations_and_keep_credit_and_token_units_separate() {
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
        let mut snapshot = Snapshot {
            observations: vec![
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
        assert_eq!(snapshot.panes[&panes.activity].rows.len(), 6);
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
        let usage = &snapshot.panes[&panes.usage].rows;
        assert_eq!(usage.len(), 2);
        assert!(usage[0].value.contains("input 0 · turn turn-1"));
        assert!(!usage[0].value.contains("output"));
        assert!(usage[1].value.contains("0.000000123 credits"));
        assert!(!usage[1].value.contains("turn "));
    }
}
