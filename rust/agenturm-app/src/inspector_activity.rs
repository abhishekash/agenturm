use agenturm_kernel::{ExtensionId, MetricRow, ObservationKind, PaneModel, Snapshot};
use agenturm_ui::{HistoryPage, RecordedTurn};
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
            .entry((row.harness_id.as_str(), session_id, turn_id))
            .or_insert_with(|| {
                let position = turns.len();
                turns.push(RecordedTurn {
                    harness_id: row.harness_id.as_str().to_owned(),
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
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
        snapshot,
        turns,
        initial_pane: None,
    }
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
    rows.extend(snapshot.observations.iter().rev().map(|row| {
        let (label, value) = match &row.kind {
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
            ObservationKind::Tool { category, result } => {
                ("Tool", format!("{category} · {result}"))
            }
            ObservationKind::CreditUsage { nanocredits } => (
                "Credit usage",
                format!("{} credits", agenturm_kernel::format_credits(*nanocredits)),
            ),
            ObservationKind::Lifecycle { state, reason_code } => (
                "Lifecycle",
                reason_code
                    .as_deref()
                    .map_or_else(|| state.clone(), |reason| format!("{state} · {reason}")),
            ),
            ObservationKind::Check { category, result } => {
                ("Check", format!("{category} · {result}"))
            }
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
        };
        let value = match row.identity.operation_id.as_deref() {
            Some(operation) => format!("{value} · operation {operation}"),
            None => value,
        };
        MetricRow {
            label: label.into(),
            value,
        }
    }));
    let pane_id = ExtensionId::parse("app.turn-activity").expect("built-in extension ID is valid");
    snapshot.panes.insert(
        pane_id.clone(),
        PaneModel {
            title: "Turn activity".into(),
            rows,
        },
    );
    HistoryPage {
        snapshot,
        turns: Vec::new(),
        initial_pane: Some(pane_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgenturmApp;
    use agenturm_kernel::{
        Observation, ObservationIdentity, ObservationPosition, ObservationQuery, ObservationScope,
        ObservationStore,
    };
    use agenturm_store::Store;
    use std::fs;

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
