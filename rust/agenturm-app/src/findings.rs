//! Findings from one bounded page of typed retained evidence.

use agenturm_kernel::native_review::{InternalReviewState, NativeRequestKind, NativeRequestState};
use agenturm_kernel::{
    ExtensionId, MetricRow, Observation, ObservationKind, ObservationPosition, ObservationScope,
    ReadSnapshot, Snapshot, TraceStatus,
};
use agenturm_store::{EvalQuery, SelectedEvalPage, SelectedEvalQuery};
use agenturm_ui::{
    EvaluationDetailEntry, FindingEvaluation, FindingGroup, FindingOccurrence, FindingSource,
    FindingsPage, TelemetrySpanEntry,
};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

// A producer event ID has meaning only within that source and native scope.
// Keep corrections pinned to the same complete scope even if other providers
// use matching IDs or clocks.
type Anchor<'a> = (
    &'a ExtensionId,
    bool,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

fn anchor(row: &Observation) -> Anchor<'_> {
    let identity = &row.identity;
    (
        &row.harness_id,
        identity.scope == ObservationScope::Session,
        identity.session_id.as_deref(),
        identity.turn_id.as_deref(),
        identity.worker_id.as_deref(),
        identity.operation_id.as_deref(),
    )
}

pub(super) fn project(
    snapshot: &Snapshot,
    source: impl Fn(&Observation, ReadSnapshot) -> Option<FindingSource>,
) -> FindingsPage {
    let mut latest = BTreeMap::new();
    for row in &snapshot.observations {
        let retained = latest
            .entry((anchor(row), row.event_id.as_str()))
            .or_insert(row);
        if (row.revision, row.observed_at_unix_ns)
            > (retained.revision, retained.observed_at_unix_ns)
        {
            *retained = row;
        }
    }
    let companions = latest
        .values()
        .copied()
        .filter_map(|row| companion(row, false))
        .collect::<BTreeSet<_>>();
    let commands = latest
        .values()
        .copied()
        .filter(|row| codex_failed_command(row))
        .filter_map(|row| Some((companion(row, true)?, row)))
        .collect::<BTreeMap<_, _>>();
    let mut groups = BTreeMap::new();
    for row in latest.into_values() {
        if companion(row, true).is_some_and(|key| companions.contains(&key)) {
            continue;
        }
        let Some((kind, severity, signal, details)) = signal(row) else {
            continue;
        };
        let key = (row.harness_id.clone(), kind, severity.to_owned(), signal);
        let protected_source = if codex_failed_command(row) || failed_span(row) {
            Some(row)
        } else if matches!(row.kind, ObservationKind::Check { .. }) {
            companion(row, false).and_then(|key| commands.get(&key).copied())
        } else {
            None
        };
        let occurrence = FindingOccurrence {
            position: ObservationPosition {
                observed_at_unix_ns: row.observed_at_unix_ns,
                harness_id: row.harness_id.as_str().into(),
                event_id: row.event_id.clone(),
                revision: row.revision,
                snapshot_id: snapshot.history_snapshot_id,
            },
            identity: row.identity.clone(),
            evaluation: None,
            details,
            source: protected_source
                .and_then(|candidate| source(candidate, snapshot.history_snapshot_id)),
        };
        groups.entry(key).or_insert_with(Vec::new).push(occurrence);
    }
    let mut groups = groups
        .into_iter()
        .map(|((source_id, kind, severity, signal), mut occurrences)| {
            occurrences.sort_unstable_by(|left, right| {
                let left = &left.position;
                let right = &right.position;
                (right.observed_at_unix_ns, &right.event_id, right.revision).cmp(&(
                    left.observed_at_unix_ns,
                    &left.event_id,
                    left.revision,
                ))
            });
            FindingGroup {
                source_id,
                kind,
                severity,
                signal,
                occurrences,
            }
        })
        .collect::<Vec<_>>();
    sort_groups(&mut groups);
    FindingsPage {
        groups,
        snapshot_id: snapshot.history_snapshot_id,
        scanned: snapshot.observations.len(),
        has_older: snapshot.observations_truncated,
        evaluation_partial: false,
    }
}

fn sort_groups(groups: &mut [FindingGroup]) {
    groups.sort_by(|left, right| {
        let order = |group: &FindingGroup| {
            let severity = match group.severity.as_str() {
                "error" => 0,
                "warning" => 1,
                _ => 2,
            };
            (
                severity,
                Reverse(group.occurrences[0].position.observed_at_unix_ns),
            )
        };
        order(left).cmp(&order(right))
    });
}

/// Query only distinct trace keys selected by the loaded evidence page.
/// Evaluation metadata is current at each read; its revision is independent
/// of the observation revision and selected trace evidence watermark.
pub(super) fn include_failed_evaluations(
    page: &mut FindingsPage,
    snapshot: &Snapshot,
    private: bool,
    read: impl Fn(&ExtensionId, &SelectedEvalQuery) -> Result<Option<SelectedEvalPage>, String>,
) {
    const MAX_EVAL_TRACES: usize = 12;
    let mut traces = BTreeSet::new();
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for row in snapshot.observations.iter().rev() {
        let ObservationKind::TraceSpan(span) = &row.kind else {
            continue;
        };
        if !row.harness_id.as_str().starts_with("otlp.")
            || row.identity.scope != ObservationScope::Session
            || !traces.insert((&row.harness_id, span.trace_id.as_str()))
        {
            continue;
        }
        if traces.len() > MAX_EVAL_TRACES {
            page.evaluation_partial = true;
            break;
        }
        let query = SelectedEvalQuery {
            trace_id: span.trace_id.clone(),
            evidence_snapshot: page.snapshot_id,
            view: EvalQuery::History {
                trace_id: span.trace_id.clone(),
                before_revision: None,
                limit: 1,
            },
        };
        let selected = match read(&row.harness_id, &query) {
            Ok(Some(selected)) => selected,
            Ok(None) => {
                page.evaluation_partial = true;
                continue;
            }
            Err(_) => {
                page.evaluation_partial = true;
                break;
            }
        };
        let Some(result) = selected.page.outcomes.into_iter().next() else {
            continue;
        };
        if result.trace_source_id != row.harness_id
            || result.outcome.trace_id != span.trace_id
            || selected.current_revision != Some(result.revision)
        {
            page.evaluation_partial = true;
            continue;
        }
        if result.outcome.passed {
            continue;
        }
        let source = result
            .detail_selection
            .filter(|_| private)
            .filter(|detail| {
                detail.trace_source_id == row.harness_id
                    && detail.trace_id == span.trace_id
                    && detail.revision == result.revision
                    && detail.snapshot_id.incarnation() == page.snapshot_id.incarnation()
                    && detail.snapshot_id.watermark() >= page.snapshot_id.watermark()
            })
            .map(|detail| {
                FindingSource::Evaluation(Box::new(EvaluationDetailEntry {
                    source: detail.trace_source_id,
                    trace_id: detail.trace_id,
                    revision: detail.revision,
                    receipt_id: detail.receipt_id,
                    snapshot_id: detail.snapshot_id,
                }))
            });
        let occurrence = FindingOccurrence {
            position: ObservationPosition {
                observed_at_unix_ns: row.observed_at_unix_ns,
                harness_id: row.harness_id.as_str().into(),
                event_id: row.event_id.clone(),
                revision: row.revision,
                snapshot_id: page.snapshot_id,
            },
            identity: agenturm_kernel::ObservationIdentity::source(),
            evaluation: Some(FindingEvaluation {
                trace_id: span.trace_id.clone(),
                revision: result.revision,
            }),
            details: vec![metric("Verdict", "Failed")],
            source,
        };
        groups
            .entry(row.harness_id.clone())
            .or_default()
            .push(occurrence);
    }
    page.groups.extend(
        groups
            .into_iter()
            .map(|(source_id, occurrences)| FindingGroup {
                source_id,
                kind: "Evaluation",
                severity: "error".into(),
                signal: "Evaluation did not pass".into(),
                occurrences,
            }),
    );
    sort_groups(&mut page.groups);
}

pub(super) fn telemetry_source(
    row: &Observation,
    snapshot_id: ReadSnapshot,
    history_identity: &str,
) -> Option<FindingSource> {
    if !failed_span(row) {
        return None;
    }
    let entry = TelemetrySpanEntry::from_observation(row, snapshot_id, history_identity)?;
    Some(FindingSource::Telemetry(Box::new(entry)))
}

fn failed_span(row: &Observation) -> bool {
    matches!(&row.kind, ObservationKind::TraceSpan(span) if span.status == TraceStatus::Error)
}

fn codex_failed_command(row: &Observation) -> bool {
    matches!(&row.kind, ObservationKind::Tool { category, result }
        if category == "shell" && failure(result))
        && row.event_id.starts_with("codex:")
        && row.event_id.ends_with(":tool")
        && row.identity.worker_id.is_none()
}

type Companion<'a> = (Anchor<'a>, &'a str, &'a str, u32, u64, &'a str);

/// Exact enrichments emitted from a single native record can replace its
/// generic tool finding. Other facts stay independent even at equal times.
fn companion(row: &Observation, generic: bool) -> Option<Companion<'_>> {
    let occurrence = &row.event_id;
    let identity = anchor(row);
    let operation = row.identity.operation_id.as_deref()?;
    let (category, result) = match (&row.kind, generic) {
        (ObservationKind::Tool { category, result }, true) => (category.as_str(), result.as_str()),
        (ObservationKind::McpCall { result, .. }, false) => ("mcp", result.as_str()),
        (ObservationKind::Check { result, .. }, false) => ("shell", result.as_str()),
        _ => return None,
    };
    let (origin, index) = if occurrence.starts_with("codex:") && matches!(category, "mcp" | "shell")
    {
        let suffix = match (generic, category) {
            (true, _) => ":tool",
            (false, "mcp") => ":mcp-call",
            (false, _) => ":check",
        };
        (occurrence.strip_suffix(suffix)?, category)
    } else if occurrence.starts_with("claude:") && category == "mcp" {
        let (origin, index) = occurrence.rsplit_once(if generic {
            ":tool-result-"
        } else {
            ":mcp-result-"
        })?;
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        (origin, index)
    } else if occurrence.strip_prefix("oc:").is_some_and(native_digest)
        && native_digest(operation)
        && matches!(category, "mcp" | "shell")
    {
        // OpenCode's native operation digest includes the stored part (or
        // inline tool) identity and is shared by its paired projections.
        (operation, category)
    } else {
        return None;
    };
    Some((
        identity,
        origin,
        index,
        row.revision,
        row.observed_at_unix_ns,
        result,
    ))
}

fn native_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

type Signal<'a> = (&'static str, &'a str, String, Vec<MetricRow>);

fn signal(row: &Observation) -> Option<Signal<'_>> {
    let observed = match &row.kind {
        ObservationKind::Diagnostic { code, severity } => (
            "Diagnostic",
            severity.as_str(),
            code.clone(),
            vec![metric("Code", code)],
        ),
        ObservationKind::Lifecycle { state, reason_code } if state == "failed" => (
            "Lifecycle",
            "error",
            reason_code.as_deref().unwrap_or("Reported failure").into(),
            reason_code
                .iter()
                .map(|reason| metric("Reason", reason))
                .collect(),
        ),
        ObservationKind::Tool { category, result } if failure(result) => (
            "Tool",
            severity(result),
            format!("{category} · {result}"),
            vec![metric("Result", result)],
        ),
        ObservationKind::Check { category, result } if result == "failed" => (
            "Check",
            "error",
            format!("{category} · failed"),
            vec![metric("Result", result)],
        ),
        ObservationKind::McpCall {
            method,
            server,
            tool,
            result,
            ..
        } if failure(result) => {
            let mut details = vec![metric("Method", method.as_str()), metric("Result", result)];
            details.extend(server.iter().map(|server| metric("Server", server)));
            details.extend(tool.iter().map(|tool| metric("Tool", tool)));
            let label = match (server, tool) {
                (Some(server), Some(tool)) => format!("{server} / {tool} · {result}"),
                _ => format!("{} · {result}", method.as_str()),
            };
            ("MCP", severity(result), label, details)
        }
        ObservationKind::TraceSpan(span) if span.status == TraceStatus::Error => {
            let mut details = vec![
                metric("Trace", &span.trace_id),
                metric("Span", &span.span_id),
            ];
            details.extend(
                span.target_source_id
                    .iter()
                    .map(|source| metric("Linked native source", source)),
            );
            ("Trace", "error", "Reported span error".into(), details)
        }
        ObservationKind::InternalReview(review)
            if review.state == InternalReviewState::Interrupted =>
        {
            let mut details = vec![metric("Review thread", &review.session_id)];
            for (label, value) in [
                ("Review turn", review.turn_id.as_deref()),
                ("Review worker", review.worker_id.as_deref()),
                ("Parent thread", review.parent_session_id.as_deref()),
            ] {
                details.extend(value.map(|value| metric(label, value)));
            }
            (
                "Review",
                "warning",
                "Internal review interrupted".into(),
                details,
            )
        }
        ObservationKind::NativeRequest(request)
            if request.state == NativeRequestState::Requested =>
        {
            let label = match request.request_kind {
                NativeRequestKind::Approval => "Approval requested",
                NativeRequestKind::UserInput => "User input requested",
            };
            (
                "Request",
                "warning",
                label.into(),
                vec![metric("Request ID", &request.request_id)],
            )
        }
        _ => return None,
    };
    Some(observed)
}

fn failure(result: &str) -> bool {
    matches!(result, "failed" | "denied")
}

fn severity(result: &str) -> &'static str {
    if result == "denied" {
        "warning"
    } else {
        "error"
    }
}

fn metric(label: &str, value: &str) -> MetricRow {
    MetricRow {
        label: label.into(),
        value: value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{McpMethod, ObservationIdentity};
    use serde_json::json;

    fn project(snapshot: &Snapshot) -> FindingsPage {
        super::project(snapshot, |_, _| None)
    }

    fn row(
        source: &str,
        id: &str,
        worker: &str,
        revision: u32,
        kind: ObservationKind,
    ) -> Observation {
        let mut identity = ObservationIdentity::session("shared-session");
        identity.turn_id = Some("shared-turn".into());
        identity.worker_id = Some(worker.into());
        identity.operation_id = Some("shared-operation".into());
        Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: id.into(),
            revision,
            identity,
            observed_at_unix_ns: 42,
            kind,
        }
    }

    #[test]
    fn failed_evaluation_findings_keep_source_trace_revision_without_inferred_ownership() {
        use agenturm_store::{EvalDetailSelection, EvalOutcome, EvalPage, RetainedEvalOutcome};
        let trace_id = "a".repeat(32);
        let make_span = |source, span: &str| {
            let span_id = span.repeat(16);
            row(
                source,
                &format!("otlp:{trace_id}:{span_id}"),
                "private-sibling-owner",
                0,
                ObservationKind::TraceSpan(agenturm_kernel::TraceSpan {
                    trace_id: trace_id.clone(),
                    span_id,
                    parent_span_id: None,
                    target_source_id: None,
                    target_operation_id: None,
                    started_at_unix_ns: 1,
                    status: TraceStatus::Ok,
                }),
            )
        };
        let snapshot = Snapshot {
            observations: vec![
                make_span("otlp.failed", "b"),
                make_span("otlp.passed", "b"),
                make_span("otlp.failed", "c"),
            ],
            history_snapshot_id: 14.into(),
            ..Snapshot::default()
        };
        let mut page = project(&snapshot);
        assert!(page.groups.is_empty());
        let reads = std::cell::Cell::new(0);
        let revision = u64::from(u32::MAX) + 1;
        include_failed_evaluations(&mut page, &snapshot, true, |source, query| {
            reads.set(reads.get() + 1);
            assert_eq!(query.trace_id, trace_id);
            assert_eq!(query.evidence_snapshot, 14.into());
            let passed = source.as_str() == "otlp.passed";
            Ok(Some(SelectedEvalPage {
                current_revision: Some(revision),
                page: EvalPage {
                    outcomes: vec![RetainedEvalOutcome {
                        trace_source_id: source.clone(),
                        revision,
                        outcome: EvalOutcome {
                            trace_id: trace_id.clone(),
                            passed,
                            task_id: None,
                            provider_model: None,
                            scores: Vec::new(),
                        },
                        detail_selection: Some(EvalDetailSelection {
                            trace_source_id: source.clone(),
                            trace_id: trace_id.clone(),
                            revision,
                            receipt_id: "d".repeat(32),
                            snapshot_id: 15.into(),
                        }),
                    }],
                    next_trace_id: None,
                    next_before_revision: None,
                },
            }))
        });
        assert_eq!(reads.get(), 2);
        assert!(!page.evaluation_partial);
        assert_eq!(page.groups.len(), 1);
        let group = &page.groups[0];
        assert_eq!(group.source_id.as_str(), "otlp.failed");
        assert_eq!(group.occurrences.len(), 1);
        let occurrence = &group.occurrences[0];
        assert_eq!(occurrence.identity, ObservationIdentity::source());
        assert_eq!(occurrence.evaluation.as_ref().unwrap().revision, revision);
        assert!(
            matches!(&occurrence.source, Some(FindingSource::Evaluation(detail))
            if detail.trace_id == trace_id && detail.revision == revision && detail.snapshot_id == 15.into())
        );
    }

    fn tool(category: &str, result: &str) -> ObservationKind {
        ObservationKind::Tool {
            category: category.into(),
            result: result.into(),
        }
    }

    #[test]
    fn findings_keep_exact_scope_and_latest_loaded_correction() {
        let left = "harness.left";
        let right = "harness.right";
        let shared = "shared-event";
        let first = row(left, shared, "worker-a", 2, tool("shell", "failed"));
        let snapshot = Snapshot {
            observations: vec![
                first.clone(),
                row(left, shared, "worker-a", 1, tool("shell", "failed")),
                row(left, shared, "worker-b", 1, tool("shell", "failed")),
                first,
                row(right, shared, "worker-a", 1, tool("shell", "failed")),
                row(left, "corrected", "worker-a", 0, tool("shell", "failed")),
                row(left, "corrected", "worker-a", 1, tool("shell", "succeeded")),
                row(
                    left,
                    "independent-check",
                    "worker-a",
                    0,
                    ObservationKind::Check {
                        category: "test".into(),
                        result: "failed".into(),
                    },
                ),
                row(
                    left,
                    "not-a-finding",
                    "worker-a",
                    0,
                    ObservationKind::Message {
                        role: "user".into(),
                    },
                ),
            ],
            history_snapshot_id: 87.into(),
            observations_truncated: true,
            ..Snapshot::default()
        };
        let page = project(&snapshot);
        assert_eq!(page.scanned, 9);
        assert!(page.has_older);
        assert_eq!(page.groups.len(), 3);
        let group = page
            .groups
            .iter()
            .find(|group| group.source_id.as_str() == left && group.kind == "Tool")
            .unwrap();
        assert_eq!(group.occurrences.len(), 2);
        let selected = &group.occurrences[0];
        assert_eq!(selected.position.harness_id, left);
        assert_eq!(selected.position.event_id, shared);
        assert_eq!(selected.position.revision, 2);
        assert_eq!(selected.position.snapshot_id, 87.into());
        assert_eq!(
            selected.identity.session_id.as_deref(),
            Some("shared-session")
        );
        assert_eq!(selected.identity.turn_id.as_deref(), Some("shared-turn"));
        assert_eq!(selected.identity.worker_id.as_deref(), Some("worker-a"));
        assert_eq!(
            group.occurrences[1].identity.worker_id.as_deref(),
            Some("worker-b")
        );
        assert_eq!(
            page.groups
                .iter()
                .find(|group| group.source_id.as_str() == right)
                .unwrap()
                .occurrences
                .len(),
            1
        );
    }

    #[test]
    fn native_exact_companions_do_not_double_count_or_merge_other_sources() {
        let tool_id = "codex:shared-native-record:tool";
        let mcp_id = "codex:shared-native-record:mcp-call";
        let left = "harness.left";
        let right = "harness.right";
        let generic_id = format!("oc:{}", "a".repeat(64));
        let specialized_id = format!("oc:{}", "b".repeat(64));
        let operation = "c".repeat(64);
        let mut generic = row(left, &generic_id, "worker-a", 4, tool("shell", "failed"));
        let mut specialized = row(
            left,
            &specialized_id,
            "worker-a",
            4,
            ObservationKind::Check {
                category: "test".into(),
                result: "failed".into(),
            },
        );
        generic.identity.operation_id = Some(operation.clone());
        specialized.identity.operation_id = Some(operation);
        let snapshot = Snapshot {
            observations: vec![
                row(left, tool_id, "worker-a", 0, tool("mcp", "denied")),
                row(
                    left,
                    mcp_id,
                    "worker-a",
                    0,
                    ObservationKind::McpCall {
                        server: Some("broker".into()),
                        tool: Some("lookup".into()),
                        method: McpMethod::ToolsCall,
                        duration_ns: Some(1),
                        result: "denied".into(),
                    },
                ),
                row(
                    left,
                    "codex:another-record:tool",
                    "worker-a",
                    0,
                    tool("mcp", "denied"),
                ),
                row(left, tool_id, "worker-b", 0, tool("mcp", "denied")),
                row(right, tool_id, "worker-a", 0, tool("mcp", "denied")),
                generic,
                specialized,
            ],
            ..Snapshot::default()
        };
        let page = project(&snapshot);
        let all = page
            .groups
            .iter()
            .flat_map(|group| &group.occurrences)
            .collect::<Vec<_>>();
        assert_eq!(all.len(), 5);
        assert!(all.iter().any(|event| event.position.event_id == mcp_id));
        assert!(
            all.iter()
                .any(|event| event.position.event_id == specialized_id)
        );
        assert!(
            all.iter()
                .all(|event| event.position.event_id != generic_id)
        );
        assert!(
            all.iter()
                .filter(|event| event.position.event_id == tool_id)
                .all(|event| {
                    event.position.harness_id == right
                        || event.identity.worker_id.as_deref() == Some("worker-b")
                })
        );
    }

    #[test]
    fn exact_codex_check_supersedes_only_its_matching_tool_projection() {
        let source = "harness.codex.fixture";
        let page = project(&Snapshot {
            observations: vec![
                row(
                    source,
                    "codex:record:9:tool",
                    "worker-a",
                    2,
                    tool("shell", "failed"),
                ),
                row(
                    source,
                    "codex:record:10:tool",
                    "worker-a",
                    2,
                    tool("shell", "failed"),
                ),
                row(
                    source,
                    "codex:record:9:tool",
                    "worker-b",
                    1,
                    tool("shell", "denied"),
                ),
                row(
                    source,
                    "codex:record:9:check",
                    "worker-a",
                    2,
                    ObservationKind::Check {
                        category: "test".into(),
                        result: "failed".into(),
                    },
                ),
            ],
            ..Snapshot::default()
        });
        assert_eq!(page.groups.len(), 3);
        assert!(page.groups.iter().all(|group| group.occurrences.len() == 1));
        assert!(page.groups.iter().any(|group| {
            group.kind == "Check"
                && group.occurrences[0].position.event_id == "codex:record:9:check"
        }));
        assert!(page.groups.iter().any(|group| {
            group.severity == "error"
                && group.kind == "Tool"
                && group.occurrences[0].position.event_id == "codex:record:10:tool"
        }));
    }

    #[test]
    fn a_check_mints_source_only_for_its_exact_retained_main_command() {
        let main = |source, id, revision, kind| {
            let mut row = row(source, id, "worker", revision, kind);
            row.identity.worker_id = None;
            row
        };
        let source = "harness.codex.fixture";
        let tool_id = "codex:record:9:tool";
        let check_id = "codex:record:9:check";
        let check = ObservationKind::Check {
            category: "test".into(),
            result: "failed".into(),
        };
        let snapshot = Snapshot {
            history_snapshot_id: 88.into(),
            observations: vec![
                main(source, tool_id, 3, tool("shell", "failed")),
                main(source, check_id, 3, check.clone()),
                main(source, "codex:record:10:check", 5, check.clone()),
                main("harness.codex.sibling", check_id, 3, check.clone()),
                row(source, check_id, "foreign-worker", 3, check),
            ],
            ..Snapshot::default()
        };
        let page = super::project(&snapshot, |command, pinned| {
            assert_eq!(pinned, 88.into());
            assert_eq!(command.event_id, tool_id);
            assert_eq!(command.harness_id.as_str(), source);
            assert_eq!(command.revision, 3);
            assert!(command.identity.worker_id.is_none());
            Some(FindingSource::Command("opaque-main-command".into()))
        });
        let occurrences = page.groups.iter().flat_map(|group| &group.occurrences);
        let selected = occurrences
            .clone()
            .filter(|row| row.source.is_some())
            .collect::<Vec<_>>();
        assert_eq!(occurrences.count(), 4);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].position.event_id, check_id);
        assert_eq!(
            selected[0].source.as_ref(),
            Some(&FindingSource::Command("opaque-main-command".into()))
        );
    }

    #[test]
    fn a_failed_span_mints_a_body_free_exact_telemetry_selection() {
        let trace_id = "11".repeat(16);
        let span_id = "33".repeat(8);
        let event_id = format!("otlp:{trace_id}:{span_id}");
        let mut span = row(
            "otlp.fixture",
            &event_id,
            "typed-worker",
            0,
            ObservationKind::TraceSpan(agenturm_kernel::TraceSpan {
                trace_id: trace_id.clone(),
                span_id: span_id.clone(),
                parent_span_id: None,
                target_source_id: Some("harness.codex.fixture".into()),
                target_operation_id: None,
                started_at_unix_ns: 8,
                status: TraceStatus::Error,
            }),
        );
        span.identity.operation_id = None;
        let mut rebound = span.clone();
        rebound.revision = 1;
        assert!(telemetry_source(&rebound, 8.into(), "one-history").is_none());
        rebound.revision = 0;
        rebound.event_id = "not-the-vault-receipt".into();
        assert!(telemetry_source(&rebound, 8.into(), "one-history").is_none());
        let mut other = span.clone();
        other.event_id = "successful-span".into();
        if let ObservationKind::TraceSpan(span) = &mut other.kind {
            span.status = TraceStatus::Ok;
        }
        assert!(telemetry_source(&other, 8.into(), "one-history").is_none());
        let page = super::project(
            &Snapshot {
                observations: vec![span, other],
                history_snapshot_id: 8.into(),
                ..Snapshot::default()
            },
            |row, snapshot| telemetry_source(row, snapshot, "one-history"),
        );
        assert_eq!(page.groups.len(), 1);
        let occurrence = &page.groups[0].occurrences[0];
        assert_eq!(occurrence.position.event_id, event_id);
        let Some(FindingSource::Telemetry(selection)) = &occurrence.source else {
            panic!("expected exact telemetry selection");
        };
        assert_eq!(selection.trace_id, trace_id);
        assert_eq!(selection.span_id, span_id);
        assert_eq!(
            selection.native_source_id.as_deref(),
            Some("harness.codex.fixture")
        );
        assert_eq!(selection.source_id.as_str(), "otlp.fixture");
        assert_eq!(selection.turn_id.as_deref(), Some("shared-turn"));
        assert_eq!(selection.worker_id.as_deref(), Some("typed-worker"));
        assert_eq!(selection.snapshot_id, 8.into());
        assert_eq!(selection.history_identity, "one-history");
    }

    #[test]
    fn review_interruptions_and_request_events_keep_exact_recorded_scope() {
        let review_source = format!("review.codex.{}", "1".repeat(64));
        let review = |event_id: &str, state: &str| {
            Observation {
            harness_id: ExtensionId::parse(&review_source).unwrap(),
            event_id: event_id.into(),
            revision: 3,
            identity: ObservationIdentity::source(),
            observed_at_unix_ns: 12,
            kind: serde_json::from_value(json!({
                "kind":"internal_review","state":state,"session_id":"review-thread",
                "turn_id":"review-turn","worker_id":"review-worker","parent_session_id":"parent-thread",
            })).unwrap(),
        }
        };
        let request = |source, request_kind| {
            row(
                source,
                &format!("{request_kind}-event"),
                "exact-worker",
                2,
                serde_json::from_value(json!({"kind":"native_request","request_id":"a".repeat(64),
                "state":"requested","request_kind":request_kind}))
                .unwrap(),
            )
        };
        let page = project(&Snapshot {
            history_snapshot_id: 42.into(),
            observations: vec![
                review("interrupted-review", "interrupted"),
                review("ordinary-completion", "completed"),
                request("harness.codex.one", "approval"),
                request("harness.codex.two", "approval"),
                request("harness.codex.one", "user_input"),
            ],
            ..Snapshot::default()
        });
        assert_eq!(page.groups.len(), 4);
        let review = page
            .groups
            .iter()
            .find(|group| group.kind == "Review")
            .unwrap();
        assert_eq!(review.source_id.as_str(), review_source);
        assert_eq!(review.signal, "Internal review interrupted");
        assert_eq!(review.severity, "warning");
        let event = &review.occurrences[0];
        assert_eq!(event.position.event_id, "interrupted-review");
        assert_eq!(event.position.revision, 3);
        assert_eq!(event.identity, ObservationIdentity::source());
        assert_eq!(
            event.details,
            [
                metric("Review thread", "review-thread"),
                metric("Review turn", "review-turn"),
                metric("Review worker", "review-worker"),
                metric("Parent thread", "parent-thread")
            ]
        );
        let requests = page
            .groups
            .iter()
            .filter(|group| group.kind == "Request")
            .collect::<Vec<_>>();
        assert_eq!(
            requests
                .iter()
                .filter(|group| group.signal == "Approval requested")
                .count(),
            2
        );
        assert_eq!(
            requests
                .iter()
                .filter(|group| group.signal == "User input requested")
                .count(),
            1
        );
        for group in requests {
            let [event] = group.occurrences.as_slice() else {
                panic!("source request group changed")
            };
            assert_eq!(event.position.harness_id, group.source_id.as_str());
            assert_eq!(
                event.position.event_id,
                match group.signal.as_str() {
                    "Approval requested" => "approval-event",
                    "User input requested" => "user_input-event",
                    _ => panic!("unexpected request kind"),
                }
            );
            assert_eq!(event.position.revision, 2);
            assert_eq!(event.position.snapshot_id, 42.into());
            assert_eq!(event.identity.worker_id.as_deref(), Some("exact-worker"));
        }
    }
}
