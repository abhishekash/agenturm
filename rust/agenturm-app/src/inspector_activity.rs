use agenturm_kernel::{MetricRow, Observation, ObservationKind, ObservationQuery, Snapshot};
use agenturm_ui::{
    HistoryPage, OperationDetailEntry, OperationDetailKind, RecordedSession, RecordedTurn,
};
use std::collections::HashMap;

pub(super) fn overview_page(snapshot: Snapshot) -> HistoryPage {
    let mut turns = Vec::<RecordedTurn>::new();
    let mut positions = HashMap::new();
    for row in snapshot.observations.iter().rev() {
        let (Some(session_id), Some(turn_id)) = (&row.identity.session_id, &row.identity.turn_id)
        else {
            continue;
        };
        let position = positions
            .entry((
                row.harness_id.as_str(),
                session_id,
                row.identity.worker_id.as_deref(),
                turn_id,
            ))
            .or_insert_with(|| {
                let position = turns.len();
                turns.push(RecordedTurn {
                    harness_id: row.harness_id.as_str().to_owned(),
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    worker_id: row.identity.worker_id.clone(),
                    snapshot_id: snapshot.history_snapshot_id,
                    lifecycle: None,
                });
                position
            });
        let turn = &mut turns[*position];
        if let ObservationKind::Lifecycle { state, reason_code } = &row.kind
            && turn
                .lifecycle
                .as_ref()
                .is_none_or(|(latest, _)| !is_turn_end(latest) && is_turn_end(state))
        {
            turn.lifecycle = Some((state.clone(), reason_code.clone()));
        }
    }
    HistoryPage {
        features: Default::default(),
        history_identity: None,
        operation_details: operation_details(&snapshot),
        private_telemetry_history: None,
        operation_tables: Default::default(),
        evidence_tables: Default::default(),
        snapshot,
        turns,
        initial_pane: None,
        review_pane: None,
    }
}

fn operation_details(snapshot: &Snapshot) -> Vec<OperationDetailEntry> {
    snapshot
        .observations
        .iter()
        .rev()
        .filter_map(operation_detail)
        .collect()
}

fn operation_detail(row: &Observation) -> Option<OperationDetailEntry> {
    if row.identity.worker_id.is_some() {
        return None;
    }
    let (kind, label) = match &row.kind {
        ObservationKind::McpCall { server, tool, .. } if row.event_id.starts_with("codex:") => (
            OperationDetailKind::Mcp,
            format!(
                "{} · {}",
                tool.as_deref().unwrap_or("MCP call"),
                server.as_deref().unwrap_or("server unavailable")
            ),
        ),
        ObservationKind::FileEdit(_)
            if row.event_id.starts_with("codex:") || row.event_id.starts_with("claude:") =>
        {
            (
                OperationDetailKind::Edit,
                crate::session_evidence::describe_observation(&row.kind).1,
            )
        }
        ObservationKind::FileChange(_)
            if row.event_id.starts_with("pi:") || row.event_id.starts_with("oc:") =>
        {
            (
                OperationDetailKind::Edit,
                crate::session_evidence::describe_observation(&row.kind).1,
            )
        }
        _ => return None,
    };
    let session = row.identity.session_id.as_ref()?;
    let turn = row.identity.turn_id.as_ref()?;
    let operation = row.identity.operation_id.as_ref()?;
    Some(OperationDetailEntry {
        kind,
        source_id: row.harness_id.as_str().to_owned(),
        session_id: session.clone(),
        turn_id: turn.clone(),
        event_id: row.event_id.clone(),
        revision: row.revision,
        operation_id: operation.clone(),
        label: format!(
            "{label} · {} · turn {}",
            operation.chars().take(8).collect::<String>(),
            turn.chars().take(8).collect::<String>()
        ),
    })
}

pub(super) fn review_edits(
    store: &impl crate::HistoryReader,
    session: &RecordedSession,
    mut query: ObservationQuery,
) -> Result<HistoryPage, String> {
    if session.history_identity.is_none()
        || session.history_identity != store.workspace_history_identity()
        || query.harness_id.as_deref() != Some(session.harness_id.as_str())
        || query.session_id.as_deref() != Some(session.session_id.as_str())
        || query.snapshot_id != Some(session.snapshot_id)
        || query
            .before
            .as_ref()
            .is_some_and(|cursor| cursor.harness_id != session.harness_id)
    {
        return Err("Review edits belong to a different history selection. Reopen Review.".into());
    }

    // Each read advances over a bounded raw range. Sparse edits may require
    // another page, even when the preceding range contained no matching edits.
    query.limit = 64;
    let mut edits = Vec::new();
    for _ in 0..4 {
        let retained = store
            .query_observations(&query)
            .map_err(|error| format!("Read Review edits failed: {error}"))?;
        if retained.snapshot_id != session.snapshot_id
            || retained.observations.len() > query.limit
            || retained.observations.iter().any(|row| {
                row.harness_id.as_str() != session.harness_id
                    || row.identity.session_id.as_deref() != Some(&session.session_id)
            })
        {
            return Err("Review edits response did not match the selected session".into());
        }
        edits.extend(
            retained
                .observations
                .iter()
                .rev()
                .filter(|row| {
                    matches!(
                        row.kind,
                        ObservationKind::FileEdit(_) | ObservationKind::FileChange(_)
                    )
                })
                .filter_map(operation_detail),
        );
        let next = retained.older_position();
        if retained.has_more && (next.is_none() || next == query.before) {
            return Err("Review edits response could not continue through retained history".into());
        }
        query.before = next;
        if query.before.is_none() || edits.len() >= query.limit {
            break;
        }
    }
    let mut page = overview_page(Snapshot {
        history_snapshot_id: session.snapshot_id,
        observations_truncated: query.before.is_some(),
        ..Snapshot::default()
    });
    page.features.continuation = query.before.map(Box::new);
    page.features.review_edit_preview = store.allows_private_capture();
    page.operation_details = edits;
    Ok(page)
}

fn is_turn_end(state: &str) -> bool {
    matches!(state, "completed" | "interrupted" | "failed")
}

pub(super) fn turn_page(mut snapshot: Snapshot, turn: &RecordedTurn) -> HistoryPage {
    let mut rows = vec![
        MetricRow {
            label: "Source".into(),
            value: turn.harness_id.clone(),
        },
        MetricRow {
            label: "Session".into(),
            value: turn.session_id.clone(),
        },
        MetricRow {
            label: "Turn".into(),
            value: turn.turn_id.clone(),
        },
    ];
    if let Some(worker) = &turn.worker_id {
        rows.push(MetricRow {
            label: "Worker".into(),
            value: worker.clone(),
        });
    }
    rows.extend(snapshot.observations.iter().rev().map(|row| {
        let (label, value) = crate::session_evidence::describe_observation(&row.kind);
        let value = match row.identity.operation_id.as_deref() {
            Some(operation) => format!("{value} · operation {operation}"),
            None => value,
        };
        MetricRow {
            label: label.into(),
            value,
        }
    }));
    let pane_id = crate::session_evidence::insert_pane(
        &mut snapshot,
        "app.turn-activity",
        "Turn activity",
        rows,
    );
    HistoryPage {
        features: Default::default(),
        history_identity: None,
        operation_details: operation_details(&snapshot),
        private_telemetry_history: None,
        operation_tables: Default::default(),
        evidence_tables: Default::default(),
        snapshot,
        turns: Vec::new(),
        initial_pane: Some(pane_id),
        review_pane: None,
    }
}

/// Turn-scoped Review: the same exact turn query, with Review rows selected.
pub(super) fn turn_review_page(snapshot: Snapshot, turn: &RecordedTurn) -> HistoryPage {
    let mut page = turn_page(snapshot, turn);
    let rows = crate::session_evidence::turn_review_rows(&page.snapshot);
    let pane = crate::session_evidence::insert_pane(
        &mut page.snapshot,
        "app.turn-review",
        "Review · exact turn",
        rows,
    );
    page.review_pane = Some(pane.clone());
    page.initial_pane = Some(pane);
    page
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgenturmApp;
    use agenturm_kernel::{
        ExtensionId, Observation, ObservationIdentity, ObservationPosition, ObservationQuery,
        ObservationScope, ObservationStore,
    };
    use agenturm_store::Store;
    use std::fs;

    #[test]
    fn provider_edit_pickers_keep_exact_occurrences_without_borrowing_mcp_support() {
        for (prefix, kind) in [
            (
                "codex:fixture",
                ObservationKind::FileEdit(agenturm_kernel::FileEdit {
                    updated: 1,
                    ..Default::default()
                }),
            ),
            (
                "claude:fixture",
                ObservationKind::FileEdit(agenturm_kernel::FileEdit {
                    updated: 1,
                    ..Default::default()
                }),
            ),
            (
                "oc:fixture",
                ObservationKind::FileChange(agenturm_kernel::FileChange {
                    files: 2,
                    digest: "a".repeat(64),
                }),
            ),
        ] {
            let mut row = Observation {
                harness_id: ExtensionId::parse("harness.fixture").unwrap(),
                event_id: prefix.into(),
                revision: 2,
                identity: ObservationIdentity {
                    turn_id: Some("exact-turn".into()),
                    operation_id: Some("exact-operation".into()),
                    ..ObservationIdentity::session("selected")
                },
                observed_at_unix_ns: 1,
                kind,
            };
            let entry = operation_detail(&row).unwrap();
            assert_eq!(entry.kind, OperationDetailKind::Edit);
            assert_eq!(entry.event_id, prefix);
            assert_eq!(entry.revision, 2);
            assert_eq!(entry.turn_id, "exact-turn");
            row.identity.worker_id = Some("worker".into());
            assert!(operation_detail(&row).is_none());
            row.identity.worker_id = None;
            row.identity.turn_id = None;
            assert!(operation_detail(&row).is_none());
        }
    }

    #[test]
    fn review_edits_page_through_sparse_history_without_crossing_scope_or_version() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("harness.codex.review-fixture").unwrap();
        let row = |timestamp, session: &str, turn: &str| Observation {
            harness_id: source.clone(),
            event_id: format!("activity-{timestamp}"),
            revision: 0,
            identity: ObservationIdentity {
                turn_id: Some(turn.into()),
                ..ObservationIdentity::session(session)
            },
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };
        let edit = |timestamp, session: &str, turn: &str| {
            let mut observation = row(timestamp, session, turn);
            observation.event_id = format!("codex:edit-{timestamp}:edit");
            observation.identity.operation_id = Some("reused-operation".into());
            observation.kind = ObservationKind::FileEdit(agenturm_kernel::FileEdit {
                added: 1,
                updated: 0,
                deleted: 0,
                moved: 0,
            });
            observation
        };
        let mut worker = edit(3, "selected", "turn-b");
        worker.identity.worker_id = Some("worker".into());
        let mut rows = vec![
            edit(1, "selected", "turn-a"),
            edit(2, "selected", "turn-b"),
            worker,
            edit(4, "sibling", "turn-a"),
        ];
        rows.extend((5..=260).map(|tick| row(tick, "selected", "turn-c")));
        store
            .commit(source.as_str(), None, "initial", &rows)
            .unwrap();
        let snapshot = store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap()
            .snapshot_id;
        let mut other_source = edit(262, "selected", "turn-a");
        other_source.harness_id = ExtensionId::parse("harness.codex.sibling").unwrap();
        store
            .commit("harness.codex.sibling", None, "other", &[other_source])
            .unwrap();
        store
            .commit(
                source.as_str(),
                Some("initial"),
                "updated",
                &[edit(261, "selected", "later")],
            )
            .unwrap();
        let mut session = RecordedSession {
            harness_id: source.as_str().into(),
            session_id: "selected".into(),
            task_title: None,
            native_context: None,
            project_path: None,
            project_origin: None,
            updated_at_unix_ns: 260,
            snapshot_id: snapshot,
            state: None,
            reason: None,
            turn_id: None,
            parent_session_id: None,
            relationship_kind: None,
            review_version: agenturm_ui::ReviewVersion {
                receipt_sequence: snapshot.watermark(),
                retained_observations: 259,
            },
            reviewed: false,
            history_identity: Some(store.history_identity().unwrap()),
            worker_id: None,
            pending_requests: Vec::new(),
        };
        let query = ObservationQuery {
            harness_id: Some(source.as_str().into()),
            session_id: Some(session.session_id.clone()),
            snapshot_id: Some(snapshot),
            ..ObservationQuery::latest(1)
        };
        let first = review_edits(&store, &session, query.clone()).unwrap();
        assert!(first.operation_details.is_empty());
        assert!(first.snapshot.observations_truncated);
        let older = ObservationQuery {
            before: first.features.continuation.map(|cursor| *cursor),
            ..query.clone()
        };
        let selected = review_edits(&store, &session, older).unwrap();
        assert!(!selected.snapshot.observations_truncated);
        assert_eq!(selected.snapshot.history_snapshot_id, snapshot);
        assert_eq!(
            selected
                .operation_details
                .iter()
                .map(|entry| (entry.turn_id.as_str(), entry.event_id.as_str()))
                .collect::<Vec<_>>(),
            [
                ("turn-b", "codex:edit-2:edit"),
                ("turn-a", "codex:edit-1:edit"),
            ]
        );
        session.history_identity = Some("different-history".into());
        assert!(review_edits(&store, &session, query).is_err());
    }

    #[test]
    fn recorded_turn_activity_pins_history_and_excludes_other_sessions_and_turns() {
        let directory = std::env::temp_dir().join(format!("agenturm-turn-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("history.sqlite");
        let source = "harness.codex";
        let kind = |state: &str| ObservationKind::Lifecycle {
            state: state.into(),
            reason_code: None,
        };
        let lifecycle = |event: &str, session: &str, turn: &str, time, state: &str| Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: event.into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some(session.into()),
                worker_id: None,
                turn_id: Some(turn.into()),
                operation_id: None,
            },
            observed_at_unix_ns: time,
            kind: kind(state),
        };
        let mut raised = lifecycle("raised", "session", "turn-1", 3, "running");
        raised.identity.operation_id = Some("create-call".into());
        raised.kind = ObservationKind::PullRequestRaised {
            repository: "owner/repo".into(),
            number: 16,
        };
        let mut message = lifecycle("message", "session", "turn-without-state", 3, "running");
        message.kind = ObservationKind::Message {
            role: "user".into(),
        };
        let mut writer = Store::open(&path).unwrap();
        writer
            .commit(
                source,
                None,
                "first",
                &[
                    lifecycle("cross-session", "other", "turn-1", 1, "completed"),
                    lifecycle("start", "session", "turn-1", 2, "running"),
                    raised,
                    message,
                    lifecycle("end", "session", "turn-1", 4, "interrupted"),
                    lifecycle("next-start", "session", "turn-2", 5, "running"),
                    lifecycle("next-end", "session", "turn-2", 6, "completed"),
                ],
            )
            .unwrap();
        let mut app = AgenturmApp::default_composition();
        let store = Store::open_read_only(&path).unwrap();
        let overview =
            overview_page(app.refresh_history_query(&store, ObservationQuery::latest(10)));
        assert_eq!(
            overview
                .turns
                .iter()
                .map(|turn| (turn.session_id.as_str(), turn.turn_id.as_str()))
                .collect::<Vec<_>>(),
            [
                ("session", "turn-2"),
                ("session", "turn-1"),
                ("session", "turn-without-state"),
                ("other", "turn-1")
            ]
        );
        assert_eq!(
            overview.turns[0].lifecycle,
            Some(("completed".into(), None))
        );
        assert_eq!(overview.turns[2].lifecycle, None);
        let turn = overview.turns[1].clone();
        assert_eq!(turn.lifecycle, Some(("interrupted".into(), None)));

        writer
            .commit(
                source,
                Some("first"),
                "second",
                &[lifecycle("future", "session", "turn-1", 7, "completed")],
            )
            .unwrap();
        let mut query = ObservationQuery {
            harness_id: Some(turn.harness_id.clone()),
            session_id: Some(turn.session_id.clone()),
            snapshot_id: Some(turn.snapshot_id),
            before: None,
            limit: 1,
        };
        let snapshot = app
            .kernel
            .refresh_turn_with_store(&store, query.clone(), &turn.turn_id);
        assert!(snapshot.failures.is_empty());
        assert!(snapshot.observations_truncated);
        let row = &snapshot.observations[0];
        assert_eq!(row.event_id, "end");
        query.before = Some(ObservationPosition {
            observed_at_unix_ns: row.observed_at_unix_ns,
            harness_id: source.into(),
            event_id: row.event_id.clone(),
            revision: row.revision,
            snapshot_id: turn.snapshot_id,
        });
        let detail = turn_page(snapshot, &turn);
        assert_eq!(
            detail.snapshot.panes[&ExtensionId::parse("app.turn-activity").unwrap()]
                .rows
                .last()
                .unwrap()
                .value,
            "interrupted"
        );
        query.limit = 2;
        let older = app
            .kernel
            .refresh_turn_with_store(&store, query, &turn.turn_id);
        assert!(older.failures.is_empty());
        assert!(!older.observations_truncated);
        assert_eq!(
            older
                .observations
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["start", "raised"]
        );
        let page = turn_page(older, &turn);
        let pane = &page.snapshot.panes[&ExtensionId::parse("app.turn-activity").unwrap()];
        assert!(pane.rows.iter().any(|row| {
            row.label == "Pull request raised"
                && row.value == "https://github.com/owner/repo/pull/16 · operation create-call"
        }));
        drop(writer);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }
}
