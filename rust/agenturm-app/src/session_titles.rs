//! Saved task labels and separate retained activity captions.

use agenturm_kernel::{ExtensionId, Observation, ObservationKind, ObservationQuery};
use agenturm_store::{SessionPage, bounded_text_preview};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::HistoryReader;

const MAX_TITLE_BYTES: usize = 256;
const ROSTER_EVIDENCE_LIMIT: usize = 256;
type SessionKey = (ExtensionId, String);

pub(super) fn load(
    store: &impl HistoryReader,
    workspace: &Path,
    page: &SessionPage,
) -> BTreeMap<SessionKey, String> {
    if page.sessions.is_empty() {
        return BTreeMap::new();
    }
    let keys = page
        .sessions
        .iter()
        .map(|row| (row.harness_id.clone(), row.session_id.clone()))
        .collect::<BTreeSet<_>>();
    let tasks = store
        .workspace_ledger(workspace)
        .and_then(|ledger| {
            ledger
                .session_task_titles(store.session_title_store())
                .map_err(|error| error.to_string())
        })
        .unwrap_or_default();
    task_titles(&keys, tasks)
}

pub(super) fn last_activity(
    store: &impl HistoryReader,
    page: &SessionPage,
) -> BTreeMap<SessionKey, String> {
    if page.sessions.is_empty() {
        return BTreeMap::new();
    }
    let keys = page
        .sessions
        .iter()
        .map(|row| (row.harness_id.clone(), row.session_id.clone()))
        .collect();
    let Ok(evidence) = store.query_observations(&ObservationQuery {
        snapshot_id: Some(page.snapshot_id),
        ..ObservationQuery::latest(ROSTER_EVIDENCE_LIMIT)
    }) else {
        return BTreeMap::new();
    };
    if evidence.snapshot_id != page.snapshot_id {
        return BTreeMap::new();
    }
    activity_captions(&keys, &evidence.observations)
}

fn task_titles(
    keys: &BTreeSet<SessionKey>,
    tasks: Vec<(ExtensionId, String, String)>,
) -> BTreeMap<SessionKey, String> {
    let mut titles = BTreeMap::new();
    for (source, session, title) in tasks {
        let key = (source, session);
        if let Some(title) = safe_title(&title).filter(|_| keys.contains(&key)) {
            titles
                .entry(key)
                .and_modify(|prior: &mut Option<String>| {
                    if prior.as_ref() != Some(&title) {
                        *prior = None;
                    }
                })
                .or_insert(Some(title));
        }
    }
    titles
        .into_iter()
        .filter_map(|(key, value)| value.map(|title| (key, title)))
        .collect()
}

fn safe_title(text: &str) -> Option<String> {
    let clean = bounded_text_preview(text.to_owned(), MAX_TITLE_BYTES).ok()?;
    if clean != text {
        return None;
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn activity_captions(
    keys: &BTreeSet<SessionKey>,
    observations: &[Observation],
) -> BTreeMap<SessionKey, String> {
    let mut captions = BTreeMap::new();
    // The retained page is chronological. Later structural work updates the
    // caption independently of the recorded native or operator title.
    for observation in observations {
        let Some(session) = observation.identity.session_id.as_ref() else {
            continue;
        };
        let key = (observation.harness_id.clone(), session.clone());
        if keys.contains(&key)
            && let Some(label) = activity_caption(&observation.kind)
        {
            captions.insert(key, label);
        }
    }
    captions
}

fn activity_caption(kind: &ObservationKind) -> Option<String> {
    Some(match kind {
        ObservationKind::PullRequestRaised { repository, number } => {
            safe_title(&format!("Pull request #{number} · {repository}"))
                .unwrap_or_else(|| format!("Pull request #{number}"))
        }
        ObservationKind::FileEdit(_) => "Recorded file edit".into(),
        ObservationKind::McpCall { tool, .. } => tool.as_deref().and_then(safe_title).map_or_else(
            || "Recorded MCP operation".into(),
            |tool| format!("MCP · {tool}"),
        ),
        ObservationKind::Check { .. } => "Recorded check".into(),
        ObservationKind::Tool { category, .. } if category == "shell" => {
            "Recorded shell activity".into()
        }
        ObservationKind::Tool { .. } => "Recorded tool activity".into(),
        ObservationKind::TraceSpan(_) => "Recorded trace".into(),
        ObservationKind::TraceProgress(_) => "Recorded trace progress".into(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ObservationIdentity, TraceSpan, TraceStatus};

    #[test]
    fn saved_titles_and_activity_captions_use_exact_source_and_session_keys() {
        let codex = ExtensionId::parse("harness.codex").unwrap();
        let pi = ExtensionId::parse("harness.pi").unwrap();
        let trace = ExtensionId::parse("otlp.collector").unwrap();
        let selected = |source: &ExtensionId| (source.clone(), "same-session".to_owned());
        let keys = BTreeSet::from([selected(&codex), selected(&pi)]);
        let titles = task_titles(
            &keys,
            vec![
                (
                    codex.clone(),
                    "same-session".into(),
                    "  Verify   retry logic  ".into(),
                ),
                (
                    codex.clone(),
                    "same-session".into(),
                    "Verify retry logic".into(),
                ),
                (
                    pi.clone(),
                    "same-session".into(),
                    "Authorization: Bearer synthetic".into(),
                ),
                (
                    pi.clone(),
                    "other-session".into(),
                    "Unrelated explicit title".into(),
                ),
            ],
        );
        assert_eq!(
            titles,
            BTreeMap::from([(selected(&codex), "Verify retry logic".into())])
        );
        assert!(
            task_titles(
                &keys,
                vec![
                    (codex.clone(), "same-session".into(), "First task".into()),
                    (
                        codex.clone(),
                        "same-session".into(),
                        "Competing task".into()
                    ),
                ]
            )
            .is_empty()
        );
        let row = |harness_id: ExtensionId, kind| Observation {
            harness_id,
            identity: ObservationIdentity::session("same-session"),
            event_id: "fixture".into(),
            revision: 0,
            observed_at_unix_ns: 1,
            kind,
        };
        let derived = activity_captions(
            &keys,
            &[
                row(
                    codex.clone(),
                    ObservationKind::Message {
                        role: "user".into(),
                    },
                ),
                row(
                    pi.clone(),
                    ObservationKind::FileEdit(agenturm_kernel::FileEdit {
                        added: 0,
                        updated: 1,
                        deleted: 0,
                        moved: 0,
                    }),
                ),
                row(
                    trace,
                    ObservationKind::TraceSpan(TraceSpan {
                        trace_id: "1".repeat(32),
                        span_id: "2".repeat(16),
                        target_source_id: Some(codex.as_str().into()),
                        parent_span_id: None,
                        started_at_unix_ns: 0,
                        status: TraceStatus::Ok,
                    }),
                ),
            ],
        );
        assert_eq!(
            derived,
            BTreeMap::from([(selected(&pi), "Recorded file edit".into())])
        );
        assert!(activity_captions(&keys, &[]).is_empty());
        assert!(safe_title("Hidden\x1b title").is_none());
    }
}
