//! Findings from one bounded page of typed retained evidence.

use agenturm_kernel::{
    ExtensionId, MetricRow, Observation, ObservationKind, ObservationPosition, ObservationScope,
    Snapshot, TraceStatus,
};
use agenturm_ui::{FindingGroup, FindingOccurrence, FindingsPage};
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

pub(super) fn project(snapshot: &Snapshot) -> FindingsPage {
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
    let mut groups = BTreeMap::new();
    for row in latest.into_values() {
        if companion(row, true).is_some_and(|key| companions.contains(&key)) {
            continue;
        }
        let Some((kind, severity, signal, details)) = signal(row) else {
            continue;
        };
        let key = (row.harness_id.clone(), kind, severity.to_owned(), signal);
        let occurrence = FindingOccurrence {
            position: ObservationPosition {
                observed_at_unix_ns: row.observed_at_unix_ns,
                harness_id: row.harness_id.as_str().into(),
                event_id: row.event_id.clone(),
                revision: row.revision,
                snapshot_id: snapshot.history_snapshot_id,
            },
            identity: row.identity.clone(),
            details,
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
    FindingsPage {
        groups,
        snapshot_id: snapshot.history_snapshot_id,
        scanned: snapshot.observations.len(),
        has_older: snapshot.observations_truncated,
    }
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
}
